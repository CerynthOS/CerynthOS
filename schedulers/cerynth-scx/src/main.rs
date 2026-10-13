// cerynth-scx: CerynthOS's first sched_ext scheduler.
//
// Built on scx_rustland_core (the same framework scx_rustland and
// scx_rlfifo use). The BPF plumbing (main.bpf.c, bpf.rs, bpf_intf.rs,
// bpf_skel.rs) is shared, generic infrastructure. This file is the
// actual scheduling *policy*.

// The BPF plumbing is vendored from scx_rustland_core and generated code;
// keep the workspace's lint rules aimed at the code we actually write.
#[allow(clippy::all, clippy::pedantic, unsafe_code)]
mod bpf_skel;
pub use bpf_skel::*;
#[allow(clippy::all, clippy::pedantic, unsafe_code)]
pub mod bpf_intf;

#[rustfmt::skip]
#[allow(clippy::all, clippy::pedantic, unsafe_code)]
mod bpf;
use std::mem::MaybeUninit;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use bpf::{BpfScheduler, DispatchedTask, QueuedTask};
use clap::Parser;
use libbpf_rs::OpenObject;
use scx_utils::UserExitInfo;
use scx_utils::libbpf_clap_opts::LibbpfOpts;

mod profile;
use profile::{Params, Profile};

mod error;
mod policy;
mod stats;
mod status;

use error::SchedError;

/// How often the dispatch loop refreshes scheduler.json's heartbeat and
/// scheduler-stats.json. cerynthd considers the scheduler unhealthy once the
/// heartbeat is more than 10 seconds old, so this leaves a comfortable
/// margin.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// If tasks are stuck in `pending` and not a single dispatch has succeeded
/// for this long, give up and exit so the kernel falls back to its default
/// scheduler right away. The kernel's own watchdog (`timeout_ms` in
/// main.bpf.c, 5s) would eject us anyway; leaving on our own first is
/// quicker and leaves a clear reason in the log.
const DISPATCH_STALL_LIMIT: Duration = Duration::from_secs(2);

/// Fault injection for `scripts/scx/fault-test.sh`: when set to N, every Nth
/// dispatch attempt is treated as failed without reaching the BPF side.
const INJECT_DISPATCH_ERRORS_ENV: &str = "CERYNTH_SCX_INJECT_DISPATCH_ERRORS";

#[derive(Parser, Debug)]
#[command(name = "cerynth-scx", about = "CerynthOS sched_ext scheduler")]
struct Cli {
    /// Which scheduling profile to run.
    #[arg(long, value_enum, default_value_t = Profile::Balanced)]
    profile: Profile,
}

struct Scheduler<'a> {
    bpf: BpfScheduler<'a>,
    profile: Profile,
    params: Params,
    nr_cpus: usize,
    started_at_ms: u64,
    counters: stats::Counters,

    /// Tasks whose dispatch failed, retried first on the next pass. A task
    /// we dequeued but never dispatched is stranded: the kernel thinks we
    /// have it, so nothing else will run it.
    pending: Vec<QueuedTask>,
    /// Set when a pass ends with tasks still pending and none dispatched;
    /// cleared as soon as one goes through.
    stalled_since: Option<Instant>,

    inject_dispatch_errors_every: Option<u64>,
    dispatch_attempts: u64,
    last_error: Option<SchedError>,
}

impl<'a> Scheduler<'a> {
    fn init(
        open_object: &'a mut MaybeUninit<OpenObject>,
        profile: Profile,
        inject_dispatch_errors_every: Option<u64>,
    ) -> Result<Self> {
        let params = profile.params();
        let open_opts = LibbpfOpts::default();
        let mut bpf = BpfScheduler::init(
            open_object,
            open_opts.clone().into_bpf_open_opts(),
            0,
            false,
            false,
            true,
            false,
            params.base_slice_ns,
            "cerynth_scx",
        )?;
        let nr_cpus = usize::try_from(*bpf.nr_online_cpus_mut())
            .unwrap_or(1)
            .max(1);
        Ok(Self {
            bpf,
            profile,
            params,
            nr_cpus,
            started_at_ms: status::now_ms(),
            counters: stats::Counters::default(),
            pending: Vec::new(),
            stalled_since: None,
            inject_dispatch_errors_every,
            dispatch_attempts: 0,
            last_error: None,
        })
    }

    fn try_dispatch(&mut self, task: &DispatchedTask) -> Result<(), SchedError> {
        self.dispatch_attempts += 1;
        if let Some(every) = self.inject_dispatch_errors_every {
            if self.dispatch_attempts % every == 0 {
                return Err(SchedError::InjectedDispatch);
            }
        }
        self.bpf.dispatch_task(task).map_err(SchedError::Dispatch)
    }

    fn dispatch_tasks(&mut self) {
        // Retries go first: they've already waited longest.
        let mut tasks = std::mem::take(&mut self.pending);
        while tasks.len() < self.params.max_batch {
            match self.bpf.dequeue_task() {
                Ok(Some(task)) => {
                    self.counters.tasks_dequeued += 1;
                    tasks.push(task);
                }
                Ok(None) => break,
                Err(errno) => {
                    self.counters.dequeue_errors += 1;
                    self.last_error = Some(SchedError::Dequeue(errno));
                    break;
                }
            }
        }
        self.counters.record_batch(tasks.len());

        let ordered = policy::order_tasks(self.profile, tasks);
        self.counters.starvation_rescues += ordered.rescued;
        let load = policy::Load {
            waiting: ordered.tasks.len(),
            nr_cpus: self.nr_cpus,
        };

        let mut dispatched_any = false;
        let mut queue = ordered.tasks.into_iter();
        while let Some(task) = queue.next() {
            let mut dispatched_task = DispatchedTask::new(&task);
            dispatched_task.slice_ns = policy::slice_for(self.profile, &task, load);
            match self.try_dispatch(&dispatched_task) {
                Ok(()) => {
                    self.counters.tasks_dispatched += 1;
                    dispatched_any = true;
                }
                Err(e) => {
                    // Most likely the ring buffer to the BPF side is full.
                    // Keep this task and everything behind it, in order,
                    // and let the BPF side drain before trying again.
                    self.counters.dispatch_errors += 1;
                    self.last_error = Some(e);
                    self.pending.push(task);
                    self.pending.extend(queue);
                    break;
                }
            }
        }

        if dispatched_any || self.pending.is_empty() {
            self.stalled_since = None;
        } else if self.stalled_since.is_none() {
            self.stalled_since = Some(Instant::now());
        }

        // A non-zero count tells the BPF side we still have work, so it
        // schedules us again promptly instead of waiting for new tasks.
        self.bpf
            .notify_complete(u64::try_from(self.pending.len()).unwrap_or(u64::MAX));
    }

    fn bpf_counters(&mut self) -> stats::BpfCounters {
        stats::BpfCounters {
            nr_online_cpus: *self.bpf.nr_online_cpus_mut(),
            nr_running: *self.bpf.nr_running_mut(),
            nr_queued: *self.bpf.nr_queued_mut(),
            nr_scheduled: *self.bpf.nr_scheduled_mut(),
            user_dispatches: *self.bpf.nr_user_dispatches_mut(),
            kernel_dispatches: *self.bpf.nr_kernel_dispatches_mut(),
            cancel_dispatches: *self.bpf.nr_cancel_dispatches_mut(),
            bounce_dispatches: *self.bpf.nr_bounce_dispatches_mut(),
            failed_dispatches: *self.bpf.nr_failed_dispatches_mut(),
            sched_congested: *self.bpf.nr_sched_congested_mut(),
        }
    }

    fn write_stats(&mut self, running: bool) {
        let bpf = self.bpf_counters();
        let snapshot = stats::StatsSnapshot::new(
            running,
            self.profile,
            self.started_at_ms,
            &self.counters,
            self.pending.len(),
            bpf,
        );
        if let Err(e) = snapshot.write() {
            eprintln!("cerynth-scx: failed to write stats file: {e}");
        }
    }

    /// Once per `HEARTBEAT_INTERVAL`: refresh both files and report errors
    /// seen since the last beat. Errors are summarized here rather than
    /// printed as they happen, so a burst of thousands can't flood the log
    /// (or block the dispatch loop on a full stderr pipe).
    fn heartbeat(&mut self, errors_reported: &mut u64) {
        self.write_stats(true);
        if let Err(e) = status::SchedulerStatus::running(self.profile, self.started_at_ms).write() {
            eprintln!("cerynth-scx: failed to refresh status file: {e}");
        }

        let errors = self.counters.dispatch_errors + self.counters.dequeue_errors;
        if errors > *errors_reported {
            if let Some(e) = self.last_error.take() {
                eprintln!(
                    "cerynth-scx: {} new error(s) (dispatch {}, dequeue {} total), latest: {e}",
                    errors - *errors_reported,
                    self.counters.dispatch_errors,
                    self.counters.dequeue_errors,
                );
            }
            *errors_reported = errors;
        }
    }

    fn run(&mut self) -> Result<UserExitInfo> {
        let mut last_heartbeat = Instant::now();
        let mut errors_reported = 0;
        let mut stalled = false;

        while !self.bpf.exited() {
            self.dispatch_tasks();

            if let Some(since) = self.stalled_since {
                if since.elapsed() >= DISPATCH_STALL_LIMIT {
                    stalled = true;
                    break;
                }
            }
            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                self.heartbeat(&mut errors_reported);
                last_heartbeat = Instant::now();
            }
        }

        println!("cerynth-scx: shutting down, handing control back to the kernel...");
        let exit_info = self.bpf.shutdown_and_report();
        self.write_stats(false);

        if stalled {
            bail!(
                "no task could be dispatched for {DISPATCH_STALL_LIMIT:?} \
                 ({} pending, last error: {}); exited so the kernel's default \
                 scheduler takes over",
                self.pending.len(),
                self.last_error
                    .as_ref()
                    .map_or_else(|| "none".to_string(), ToString::to_string),
            );
        }
        exit_info
    }
}

fn inject_dispatch_errors_from_env() -> Result<Option<u64>> {
    let Ok(value) = std::env::var(INJECT_DISPATCH_ERRORS_ENV) else {
        return Ok(None);
    };
    match value.trim().parse::<u64>() {
        Ok(0) => Ok(None),
        Ok(every) => Ok(Some(every)),
        Err(e) => bail!("{INJECT_DISPATCH_ERRORS_ENV}={value:?} is not a number: {e}"),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let inject_every = inject_dispatch_errors_from_env()?;

    println!(
        "cerynth-scx: starting (profile = {}, base time slice = {} ns)",
        cli.profile.as_str(),
        cli.profile.slice_ns()
    );
    if let Some(every) = inject_every {
        eprintln!(
            "cerynth-scx: FAULT INJECTION ON - every {every}th dispatch will fail \
             ({INJECT_DISPATCH_ERRORS_ENV}). Testing only."
        );
    }

    let mut open_object = MaybeUninit::uninit();
    let result = loop {
        // An init failure (e.g. another sched_ext scheduler is already
        // loaded) returns here without touching the status file: it may
        // belong to that other, still-running instance.
        let mut sched = Scheduler::init(&mut open_object, cli.profile, inject_every)?;
        sched.write_stats(true);
        if let Err(e) = status::SchedulerStatus::running(cli.profile, sched.started_at_ms).write() {
            eprintln!("cerynth-scx: failed to write status file: {e}");
        }
        match sched.run() {
            Ok(exit_info) if exit_info.should_restart() => {}
            Ok(_) => break Ok(()),
            Err(e) => break Err(e),
        }
    };

    if let Err(e) = status::SchedulerStatus::stopped().write() {
        eprintln!("cerynth-scx: failed to write status file: {e}");
    }
    result
}
