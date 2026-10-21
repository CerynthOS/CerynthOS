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
use std::process::ExitCode;
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
mod process;
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

/// Exit code for "this start didn't work out, but starting again is expected
/// to" (`EX_TEMPFAIL` in sysexits.h). Used in two cases:
///
/// - The load failed before the scheduler ever ran. Loading can fail for
///   reasons that have nothing to do with us and are gone a moment later:
///   the kernel sets up per-task storage for every task on the machine while
///   enabling a scheduler, and under heavy load one of those small
///   allocations occasionally fails (`ops.init_task() failed (-12)`, seen
///   once in ~600 loads).
/// - The kernel asked for a restart, e.g. because a CPU went on/offline.
///
/// We can't do the second start ourselves. Loading installs a filter that
/// makes every later `mmap()` in this process fail (`scx_rustland_core` does
/// that so the dispatch loop can never page-fault). The filter cannot be
/// removed and survives `exec`, and both loading BPF and starting any
/// program at all need `mmap()`. Only a fresh process started by someone
/// else can load again, and that someone is whoever started us (cerynthd).
const EXIT_START_AGAIN: u8 = 75;

/// Fault injection for `scripts/scx/fault-test.sh`: when set to a non-zero
/// number, this start is treated as a failed load, right after the BPF
/// scheduler was attached.
const INJECT_LOAD_FAILURE_ENV: &str = "CERYNTH_SCX_INJECT_LOAD_FAILURE";

/// Set to keep core dumps on a crash (off by default, see
/// `process::disable_core_dumps`). For debugging a crash only.
const ALLOW_COREDUMP_ENV: &str = "CERYNTH_SCX_ALLOW_COREDUMP";

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

    /// Whether the dispatch loop got to run at all. If the kernel had
    /// already thrown the scheduler out before the first pass, the *load*
    /// failed, which is worth retrying; a failure later on is not.
    ran_a_pass: bool,
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
            ran_a_pass: false,
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
            self.ran_a_pass = true;
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

/// Read a non-negative number from an environment variable. Unset means
/// `None`; anything that isn't a number is an error rather than a silent
/// default, since these switches change how the scheduler behaves.
fn number_from_env<T>(name: &str) -> Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let Ok(value) = std::env::var(name) else {
        return Ok(None);
    };
    match value.trim().parse::<T>() {
        Ok(number) => Ok(Some(number)),
        Err(e) => bail!("{name}={value:?} is not a number: {e}"),
    }
}

/// What to do after the scheduler stopped.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// Done; exit with the scheduler's own result.
    Exit,
    /// Exit with `EXIT_START_AGAIN`: a fresh process is expected to work.
    StartAgain,
}

/// Pure decision, so the rules can be unit tested without a kernel.
fn decide_next(failed: bool, restart_requested: bool, ran_a_pass: bool) -> Next {
    if failed {
        // Only a failure before the first dispatch pass is a failed *load*.
        // A scheduler that ran and was then thrown out (watchdog, SysRq-S,
        // its own stall bail-out) must stay out: asking to be loaded again
        // would fight whatever stopped it.
        if ran_a_pass {
            Next::Exit
        } else {
            Next::StartAgain
        }
    } else if restart_requested {
        Next::StartAgain
    } else {
        Next::Exit
    }
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    // 0 means "off" for both switches.
    let inject_every = number_from_env::<u64>(INJECT_DISPATCH_ERRORS_ENV)?.filter(|n| *n > 0);
    let inject_load_failure =
        number_from_env::<u32>(INJECT_LOAD_FAILURE_ENV)?.is_some_and(|n| n > 0);

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
    if inject_load_failure {
        eprintln!(
            "cerynth-scx: FAULT INJECTION ON - this start will be treated as a failed load \
             ({INJECT_LOAD_FAILURE_ENV}). Testing only."
        );
    }

    if std::env::var_os(ALLOW_COREDUMP_ENV).is_none() {
        if let Err(e) = process::disable_core_dumps() {
            eprintln!("cerynth-scx: could not disable core dumps: {e}");
        }
    }

    let mut open_object = MaybeUninit::uninit();
    // An init failure (e.g. another sched_ext scheduler is already loaded)
    // returns here without touching the status file: it may belong to that
    // other, still-running instance.
    let mut sched = Scheduler::init(&mut open_object, cli.profile, inject_every)?;

    // The helper thread exists from init onwards; without this a hard kill
    // leaves the machine stalled until the kernel watchdog steps in.
    if let Err(e) = process::shield_helper_threads() {
        eprintln!(
            "cerynth-scx: could not move helper threads out of sched_ext ({e}); \
             after a crash the fallback may take until the 5s kernel watchdog"
        );
    }

    let outcome = if inject_load_failure {
        Err(anyhow::anyhow!("load failure (injected fault)"))
    } else {
        sched.write_stats(true);
        if let Err(e) = status::SchedulerStatus::running(cli.profile, sched.started_at_ms).write() {
            eprintln!("cerynth-scx: failed to write status file: {e}");
        }
        sched.run()
    };
    let ran_a_pass = sched.ran_a_pass;
    // Detach from the kernel before anything else happens.
    drop(sched);

    if let Err(e) = status::SchedulerStatus::stopped().write() {
        eprintln!("cerynth-scx: failed to write status file: {e}");
    }

    let restart_requested = matches!(&outcome, Ok(exit_info) if exit_info.should_restart());
    match decide_next(outcome.is_err(), restart_requested, ran_a_pass) {
        Next::Exit => outcome.map(|_| ExitCode::SUCCESS),
        Next::StartAgain => {
            match &outcome {
                Err(e) => eprintln!("cerynth-scx: loading failed before the scheduler ran: {e:#}"),
                Ok(_) => eprintln!("cerynth-scx: the kernel asked for a restart"),
            }
            eprintln!(
                "cerynth-scx: exiting with code {EXIT_START_AGAIN} (start me again); \
                 this process cannot load a scheduler a second time"
            );
            Ok(ExitCode::from(EXIT_START_AGAIN))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_exit_just_exits() {
        assert_eq!(decide_next(false, false, true), Next::Exit);
    }

    #[test]
    fn kernel_restart_request_asks_to_be_started_again() {
        assert_eq!(decide_next(false, true, true), Next::StartAgain);
    }

    #[test]
    fn failed_load_asks_to_be_started_again() {
        assert_eq!(decide_next(true, false, false), Next::StartAgain);
    }

    #[test]
    fn failure_after_running_is_final() {
        // Watchdog ejection, SysRq-S, our own stall bail-out: the scheduler
        // was running and something stopped it.
        assert_eq!(decide_next(true, false, true), Next::Exit);
        // Even if the kernel's exit record also carries a restart request.
        assert_eq!(decide_next(true, true, true), Next::Exit);
    }
}
