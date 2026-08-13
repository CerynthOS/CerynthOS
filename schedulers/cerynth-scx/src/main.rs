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

mod error;
mod policy;
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
}

struct Scheduler<'a> {
    bpf: BpfScheduler<'a>,
    profile: Profile,
    started_at_ms: u64,
}

impl<'a> Scheduler<'a> {
    fn init(
        open_object: &'a mut MaybeUninit<OpenObject>,
        slice_ns: u64,
        profile: Profile,
    ) -> Result<Self> {
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
                Err(errno) => {
                    eprintln!("cerynth-scx: {}", error::SchedError::Dequeue(errno));
                    break;
                }
            }
        }
        for task in policy::order_tasks(self.profile, tasks) {
            let dispatched_task = DispatchedTask::new(&task);
            if let Err(e) = self.bpf.dispatch_task(&dispatched_task) {
                eprintln!("cerynth-scx: {}", error::SchedError::Dispatch(e));
            }
        }
        self.bpf.notify_complete(0);
    }

    fn run(&mut self) -> Result<UserExitInfo> {
        let mut last_heartbeat = Instant::now();
        while !self.bpf.exited() {
            self.dispatch_tasks();
            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                if let Err(e) =
                    status::SchedulerStatus::running(self.profile, self.started_at_ms).write()
                {
                    eprintln!("cerynth-scx: failed to refresh status file: {e}");
                }
                last_heartbeat = Instant::now();
            }
        }
        println!("cerynth-scx: shutting down, handing control back to the kernel...");
        self.bpf.shutdown_and_report()
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let slice_ns: u64 = cli.profile.slice_ns();

    println!(
        "cerynth-scx: starting (profile = {:?}, time slice = {} ns)",
        cli.profile, slice_ns
    );

    let mut open_object = MaybeUninit::uninit();
    loop {
        let mut sched = Scheduler::init(&mut open_object, slice_ns, cli.profile)?;
        if let Err(e) = status::SchedulerStatus::running(cli.profile, sched.started_at_ms).write() {
            eprintln!("cerynth-scx: failed to write status file: {e}");
        }
        if !sched.run()?.should_restart() {
            break;
        }
    }
    if let Err(e) = status::SchedulerStatus::stopped().write() {
        eprintln!("cerynth-scx: failed to write status file: {e}");
    }
    Ok(())
}
