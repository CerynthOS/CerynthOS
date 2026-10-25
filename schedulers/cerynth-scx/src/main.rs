// cerynth-scx: CerynthOS's first sched_ext scheduler.
//
// Built on scx_rustland_core (the same framework scx_rustland and
// scx_rlfifo use). The BPF plumbing (main.bpf.c, bpf.rs, bpf_intf.rs,
// bpf_skel.rs) is shared, generic infrastructure. This file is the
// actual scheduling *policy*.

mod bpf_skel;
pub use bpf_skel::*;
pub mod bpf_intf;

#[rustfmt::skip]
mod bpf;
use std::mem::MaybeUninit;
use std::time::{Duration, Instant};

use anyhow::Result;
use bpf::*;
use clap::Parser;
use libbpf_rs::OpenObject;
use scx_utils::UserExitInfo;
use scx_utils::libbpf_clap_opts::LibbpfOpts;

mod profile;
use profile::Profile;

mod policy;
mod reporter;
mod initialization;
mod status;

const MAX_BATCH: usize = 64;

/// How often the dispatch loop refreshes scheduler.json's heartbeat.
/// cerynthd considers the scheduler unhealthy once the heartbeat is more
/// than 10 seconds old, so this leaves a comfortable margin.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Parser, Debug)]
#[command(name = "cerynth-scx", about = "CerynthOS sched_ext scheduler")]
struct Cli {
    /// Which schedulinh profile to run.
    #[arg(long, value_enum, default_value_t = Profile::Balanced)]
    profile: Profile,
    #[arg(long, hide = true, requires = "reporter_parent")]
    status_reporter: bool,
    #[arg(long, hide = true)]
    reporter_parent: Option<u32>,
}

struct Scheduler<'a> {
    bpf: BpfScheduler<'a>,
    profile: Profile,
    started_at_ms: u64,
    stats: status::SchedulerStats,
    // Field order matters: detach BPF before Reporter::drop can wait.
    reporter: reporter::Reporter,
}

impl<'a> Scheduler<'a> {
    fn init(
        open_object: &'a mut MaybeUninit<OpenObject>,
        slice_ns: u64,
        profile: Profile,
    ) -> Result<Self> {
        let reporter = reporter::Reporter::start()?;
        let open_opts = LibbpfOpts::default();
        let bpf = BpfScheduler::init(
            open_object,
            open_opts.clone().into_bpf_open_opts(),
            0,
            false,
            false,
            true,
            false,
            slice_ns,
            "cerynth_scx",
        )?;
        let started_at_ms = status::now_ms();
        Ok(Self {
            bpf,
            profile,
            started_at_ms,
            reporter,
            stats: status::SchedulerStats {
                pid: std::process::id(),
                profile: format!("{profile:?}"),
                ..Default::default()
            },
        })
    }

    fn dispatch_tasks(&mut self) {
        let mut tasks = Vec::new();
        loop {
            if tasks.len() >= MAX_BATCH {
                break;
            }
            match self.bpf.dequeue_task() {
                Ok(Some(task)) => tasks.push(task),
                Ok(None) => break,
                Err(_) => {
                    self.stats.dequeue_errors += 1;
                    break;
                }
            }
        }
        let batch_size = tasks.len() as u64;
        self.stats.tasks_dequeued += batch_size;
        if batch_size > 0 {
            self.stats.batches_processed += 1;
            self.stats.max_batch_size = self.stats.max_batch_size.max(batch_size);
            self.stats.avg_batch_size =
                self.stats.tasks_dequeued as f64 / self.stats.batches_processed as f64;
        }
        for task in policy::order_tasks(self.profile, tasks) {
            let dispatched_task = DispatchedTask::new(&task);
            if self.bpf.dispatch_task(&dispatched_task).is_err() {
                self.stats.dispatch_errors += 1;
            } else {
                self.stats.tasks_dispatched += 1;
            }
        }
        self.bpf.notify_complete(0);
        self.stats.progress_sequence += 1;
    }

    fn write_stats(&mut self) {
        self.stats.heartbeat = status::now_ms() / 1000;
        self.stats.scheduler_uptime_ms = status::now_ms().saturating_sub(self.started_at_ms);
        self.stats.kernel_queued = *self.bpf.nr_queued_mut();
        self.stats.kernel_scheduled = *self.bpf.nr_scheduled_mut();
        self.stats.kernel_user_dispatches = *self.bpf.nr_user_dispatches_mut();
        self.stats.kernel_direct_dispatches = *self.bpf.nr_kernel_dispatches_mut();
        self.stats.kernel_failed_dispatches = *self.bpf.nr_failed_dispatches_mut();
        self.stats.kernel_cancel_dispatches = *self.bpf.nr_cancel_dispatches_mut();
        if !matches!(
            self.reporter
                .publish(self.profile, self.started_at_ms, &self.stats),
            Ok(true)
        ) {
            self.stats.reporter_drops += 1;
        }
    }

    fn run(&mut self) -> Result<UserExitInfo> {
        let mut last_heartbeat = Instant::now();
        while !self.bpf.exited() {
            self.dispatch_tasks();
            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                self.write_stats();
                last_heartbeat = Instant::now();
            }
        }
        self.bpf.shutdown_and_report()
    }
}

fn run_scheduler(cli: Cli) -> Result<()> {
    let slice_ns: u64 = cli.profile.slice_ns();

    println!(
        "cerynth-scx: starting (profile = {:?}, time slice = {} ns)",
        cli.profile, slice_ns
    );

    let mut open_object = MaybeUninit::uninit();
    loop {
        let mut sched = Scheduler::init(&mut open_object, slice_ns, cli.profile)?;
        sched.write_stats();
        if !sched.run()?.should_restart() {
            break;
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.status_reporter {
        return Ok(reporter::run(
            cli.reporter_parent.expect("required by clap"),
        )?);
    }
    let result = run_scheduler(cli);
    // A watchdog/BPF error returns through `?` above. Publish the stopped
    // state on that path as well as on an ordinary shutdown.
    let mut stopped = status::SchedulerStatus::stopped();
    stopped.exit_error = result.as_ref().err().map(|error| format!("{error:#}"));
    if let Err(e) = stopped.write() {
        eprintln!("cerynth-scx: failed to write status file: {e}");
    }
    result
}
