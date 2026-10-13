use std::path::Path;

use serde::Serialize;

use crate::profile::Profile;
use crate::status::{self, write_json_atomic};

const STATS_PATH: &str = "/run/cerynth/scheduler-stats.json";

/// Bump this when a field is renamed, removed or changes meaning. Adding a
/// field does not need a bump: readers must ignore fields they don't know.
pub const SCHEMA_VERSION: u32 = 1;

/// Counters kept by the user-space dispatch loop. Plain integers: the loop
/// is single-threaded, so there is nothing to synchronize, and bumping one
/// costs about as much as the comparison next to it.
#[derive(Default, Debug)]
pub struct Counters {
    pub tasks_dequeued: u64,
    pub tasks_dispatched: u64,
    pub dispatch_errors: u64,
    pub dequeue_errors: u64,
    pub starvation_rescues: u64,
    batches_processed: u64,
    batch_tasks_total: u64,
    max_batch_size: u64,
}

impl Counters {
    /// Record one dispatch pass that had `size` tasks to place (newly
    /// dequeued plus any retried from an earlier failed dispatch). Empty
    /// passes are not batches: the loop spins through plenty of those while
    /// idle and counting them would drag `avg_batch_size` toward zero.
    pub fn record_batch(&mut self, size: usize) {
        if size == 0 {
            return;
        }
        let size = u64::try_from(size).unwrap_or(u64::MAX);
        self.batches_processed += 1;
        self.batch_tasks_total = self.batch_tasks_total.saturating_add(size);
        self.max_batch_size = self.max_batch_size.max(size);
    }

    #[allow(clippy::cast_precision_loss)] // counts far below 2^52 in practice
    fn avg_batch_size(&self) -> f64 {
        if self.batches_processed == 0 {
            return 0.0;
        }
        let avg = self.batch_tasks_total as f64 / self.batches_processed as f64;
        (avg * 100.0).round() / 100.0
    }
}

/// Counters the BPF half keeps in its own memory (`main.bpf.c`), copied out
/// once per snapshot. These see things user space can't, e.g. tasks the
/// kernel side dispatched straight to an idle CPU without asking us.
#[derive(Serialize, Default, Debug, Clone, Copy)]
pub struct BpfCounters {
    pub nr_online_cpus: u64,
    /// Tasks on a CPU right now.
    pub nr_running: u64,
    /// Tasks waiting in the ring buffer for user space to pick them up.
    pub nr_queued: u64,
    /// Tasks user space has taken but not yet dispatched.
    pub nr_scheduled: u64,
    /// Dispatches user space made to the CPU it chose.
    pub user_dispatches: u64,
    /// Dispatches the BPF side made on its own (e.g. straight to an idle CPU).
    pub kernel_dispatches: u64,
    /// Dispatches dropped because the task changed state in the meantime.
    pub cancel_dispatches: u64,
    /// Dispatches redirected because the chosen CPU wasn't allowed.
    pub bounce_dispatches: u64,
    pub failed_dispatches: u64,
    /// Times the ring buffer to user space was full.
    pub sched_congested: u64,
}

/// `/run/cerynth/scheduler-stats.json`. Frozen interface - see
/// `docs/scheduler/stats.md` before changing anything here.
#[derive(Serialize, Debug)]
pub struct StatsSnapshot {
    pub schema_version: u32,
    pub running: bool,
    pub pid: u32,
    pub profile: &'static str,
    pub started_at_ms: u64,
    pub uptime_ms: u64,
    /// Same meaning as `heartbeat` in scheduler.json: Unix seconds of this
    /// write.
    pub heartbeat: u64,
    pub tasks_dequeued: u64,
    pub tasks_dispatched: u64,
    /// Tasks held over for retry after a failed dispatch.
    pub tasks_pending: u64,
    pub dispatch_errors: u64,
    pub dequeue_errors: u64,
    pub batches_processed: u64,
    pub avg_batch_size: f64,
    pub max_batch_size: u64,
    pub starvation_rescues: u64,
    pub bpf: BpfCounters,
}

impl StatsSnapshot {
    pub fn new(
        running: bool,
        profile: Profile,
        started_at_ms: u64,
        counters: &Counters,
        tasks_pending: usize,
        bpf: BpfCounters,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            running,
            pid: std::process::id(),
            profile: profile.as_str(),
            started_at_ms,
            uptime_ms: status::now_ms().saturating_sub(started_at_ms),
            heartbeat: status::now_secs(),
            tasks_dequeued: counters.tasks_dequeued,
            tasks_dispatched: counters.tasks_dispatched,
            tasks_pending: u64::try_from(tasks_pending).unwrap_or(u64::MAX),
            dispatch_errors: counters.dispatch_errors,
            dequeue_errors: counters.dequeue_errors,
            batches_processed: counters.batches_processed,
            avg_batch_size: counters.avg_batch_size(),
            max_batch_size: counters.max_batch_size,
            starvation_rescues: counters.starvation_rescues,
            bpf,
        }
    }

    pub fn write(&self) -> std::io::Result<()> {
        write_json_atomic(Path::new(STATS_PATH), self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_passes_are_not_batches() {
        let mut c = Counters::default();
        c.record_batch(0);
        c.record_batch(0);
        assert_eq!(c.batches_processed, 0);
        assert!(c.avg_batch_size().abs() < f64::EPSILON);
    }

    #[test]
    fn batch_average_and_max() {
        let mut c = Counters::default();
        c.record_batch(10);
        c.record_batch(0);
        c.record_batch(5);
        c.record_batch(6);
        assert_eq!(c.batches_processed, 3);
        assert_eq!(c.max_batch_size, 10);
        assert!((c.avg_batch_size() - 7.0).abs() < f64::EPSILON);
    }

    #[test]
    fn average_is_rounded_to_two_decimals() {
        let mut c = Counters::default();
        c.record_batch(1);
        c.record_batch(1);
        c.record_batch(2);
        assert!((c.avg_batch_size() - 1.33).abs() < f64::EPSILON);
    }

    // The schema is a frozen interface other tracks build against. If this
    // test needs changing, so does docs/scheduler/stats.md, and anyone
    // reading the file needs to hear about it.
    #[test]
    fn snapshot_field_names_are_frozen() {
        let snap = StatsSnapshot::new(
            true,
            Profile::Interactive,
            status::now_ms(),
            &Counters::default(),
            0,
            BpfCounters::default(),
        );
        let json = serde_json::to_value(&snap).unwrap();

        let mut top: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        top.sort_unstable();
        assert_eq!(
            top,
            [
                "avg_batch_size",
                "batches_processed",
                "bpf",
                "dequeue_errors",
                "dispatch_errors",
                "heartbeat",
                "max_batch_size",
                "pid",
                "profile",
                "running",
                "schema_version",
                "started_at_ms",
                "starvation_rescues",
                "tasks_dequeued",
                "tasks_dispatched",
                "tasks_pending",
                "uptime_ms",
            ]
        );

        let mut bpf: Vec<&str> = json["bpf"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        bpf.sort_unstable();
        assert_eq!(
            bpf,
            [
                "bounce_dispatches",
                "cancel_dispatches",
                "failed_dispatches",
                "kernel_dispatches",
                "nr_online_cpus",
                "nr_queued",
                "nr_running",
                "nr_scheduled",
                "sched_congested",
                "user_dispatches",
            ]
        );

        assert_eq!(json["profile"], "interactive");
        assert_eq!(json["schema_version"], SCHEMA_VERSION);
    }
}
