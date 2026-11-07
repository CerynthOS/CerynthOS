use crate::bpf::QueuedTask;
use crate::profile::Profile;

/// A task that has run less than this since it last slept counts as a
/// "short burst" - the shape of foreground work (a keypress, a redraw, a
/// request handler) rather than a CPU hog. Matches Interactive's base slice:
/// a task that hasn't even used one interactive slice yet is not a hog.
const SHORT_BURST_NS: u64 = 2_000_000; // 2ms

/// Balanced scales its slice by task weight (nice value) but never outside
/// this range, so a nice -20 task can't monopolize a CPU and a nice 19 task
/// still gets enough time to make progress.
const BALANCED_MIN_SLICE_NS: u64 = 1_000_000; // 1ms
const BALANCED_MAX_SLICE_NS: u64 = 20_000_000; // 20ms

/// Interactive shrinks its slice as the queue grows, but never below this:
/// past a point, more context switches cost more than they buy in latency.
const INTERACTIVE_MIN_SLICE_NS: u64 = 500_000; // 0.5ms

/// The weight the kernel gives a nice-0 task.
const DEFAULT_WEIGHT: u64 = 100;

/// How busy the machine is right now, as far as slice sizing cares.
#[derive(Clone, Copy, Debug)]
pub struct Load {
    /// Tasks being dispatched in this pass.
    pub waiting: usize,
    pub nr_cpus: usize,
}

/// A batch in dispatch order, plus how many of its tasks took the
/// starvation fast lane (reported as `starvation_rescues` in the stats file).
pub struct Ordered {
    pub tasks: Vec<QueuedTask>,
    pub rescued: u64,
}

/// Decide dispatch order for a batch of waiting tasks. Pure: no BPF, no
/// kernel calls - just a list in, a list out - which is what makes it
/// unit-testable without loading a real scheduler at all.
///
/// - Balanced, Performance: FIFO (arrival order), never reordered.
/// - Interactive: starved tasks first, then shortest `exec_runtime` first.
/// - Background: starved tasks first, then short bursts, then CPU hogs,
///   each lane in arrival order.
pub fn order_tasks(profile: Profile, mut tasks: Vec<QueuedTask>) -> Ordered {
    let Some(threshold) = profile.params().starvation_threshold_ns else {
        return Ordered { tasks, rescued: 0 };
    };

    // Every task in one batch was dequeued together, so the newest `stop_ts`
    // in the batch stands in for "now" - no wall-clock reading needed, which
    // also avoids comparing against a clock that isn't guaranteed to be in
    // the same domain as these kernel-side timestamps.
    let now = tasks.iter().map(|task| task.stop_ts).max().unwrap_or(0);
    let starved = |task: &QueuedTask| {
        // stop_ts == 0 means the task has never run, so there is no "time
        // since it last ran" to measure. New tasks already sort early on
        // their zero exec_runtime; counting them as rescues would only
        // inflate the counter.
        task.stop_ts != 0 && now.saturating_sub(task.stop_ts) >= threshold
    };
    let rescued = tasks.iter().filter(|task| starved(task)).count();

    // sort_by_key is stable: tasks with equal keys keep arrival order.
    match profile {
        Profile::Interactive => tasks.sort_by_key(|task| {
            if starved(task) {
                (0, u64::MAX - now.saturating_sub(task.stop_ts))
            } else {
                (1, task.exec_runtime)
            }
        }),
        Profile::Background => tasks.sort_by_key(|task| {
            if starved(task) {
                0
            } else if is_short_burst(task) {
                1
            } else {
                2
            }
        }),
        Profile::Balanced | Profile::Performance => {}
    }

    Ordered {
        tasks,
        rescued: u64::try_from(rescued).unwrap_or(u64::MAX),
    }
}

/// Time slice for one task, always non-zero.
///
/// It has to be explicit: when the BPF side is handed a slice of 0, the
/// kernel does *not* substitute a default - it keeps whatever the task had
/// left, and if that was used up, gives it a 1ns slice
/// (`p->scx.slice = p->scx.slice ?: 1` in `kernel/sched/ext.c`). A CPU hog
/// would then be preempted on the very next tick no matter which profile
/// was running.
///
/// - Balanced: base slice scaled by weight (nice), clamped to 1-20ms.
/// - Interactive: base slice divided by the per-CPU queue depth, so the
///   more tasks are waiting, the sooner each one gets a turn (floor 0.5ms).
/// - Performance: fixed base slice. Predictable, few context switches.
/// - Background: short bursts get Balanced's slice so foreground-like work
///   isn't held to a 20ms cadence; CPU hogs get the full long slice.
pub fn slice_for(profile: Profile, task: &QueuedTask, load: Load) -> u64 {
    let base = profile.params().base_slice_ns;
    match profile {
        Profile::Balanced => {
            let weight = task.weight.max(1);
            (base.saturating_mul(weight) / DEFAULT_WEIGHT)
                .clamp(BALANCED_MIN_SLICE_NS, BALANCED_MAX_SLICE_NS)
        }
        Profile::Interactive => {
            let per_cpu = load.waiting.div_ceil(load.nr_cpus.max(1)).max(1);
            let per_cpu = u64::try_from(per_cpu).unwrap_or(u64::MAX);
            (base / per_cpu).max(INTERACTIVE_MIN_SLICE_NS)
        }
        Profile::Performance => base,
        Profile::Background => {
            if is_short_burst(task) {
                Profile::Balanced.params().base_slice_ns
            } else {
                base
            }
        }
    }
}

fn is_short_burst(task: &QueuedTask) -> bool {
    task.exec_runtime < SHORT_BURST_NS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_task(pid: i32, exec_runtime: u64) -> QueuedTask {
        fake_task_with_stop_ts(pid, exec_runtime, 0)
    }

    fn fake_task_with_stop_ts(pid: i32, exec_runtime: u64, stop_ts: u64) -> QueuedTask {
        QueuedTask {
            pid,
            cpu: 0,
            nr_cpus_allowed: 1,
            flags: 0,
            start_ts: 0,
            stop_ts,
            exec_runtime,
            weight: 100,
            vtime: 0,
            enq_cnt: 0,
            comm: [0; 16],
        }
    }

    fn fake_task_with_weight(weight: u64) -> QueuedTask {
        QueuedTask {
            weight,
            ..fake_task(1, 0)
        }
    }

    fn pids(ordered: &Ordered) -> Vec<i32> {
        ordered.tasks.iter().map(|t| t.pid).collect()
    }

    const IDLE: Load = Load {
        waiting: 1,
        nr_cpus: 4,
    };

    #[test]
    fn balanced_preserves_arrival_order() {
        let tasks = vec![fake_task(1, 500), fake_task(2, 10), fake_task(3, 9999)];
        let ordered = order_tasks(Profile::Balanced, tasks);
        assert_eq!(pids(&ordered), vec![1, 2, 3]);
        assert_eq!(ordered.rescued, 0);
    }

    #[test]
    fn interactive_sorts_by_exec_runtime_ascending() {
        let tasks = vec![fake_task(1, 500), fake_task(2, 10), fake_task(3, 9999)];
        let ordered = order_tasks(Profile::Interactive, tasks);
        assert_eq!(pids(&ordered), vec![2, 1, 3]);
    }

    #[test]
    fn interactive_rescues_a_long_waiting_task() {
        // pid 3 has a huge exec_runtime (would normally sort last) but its
        // stop_ts is far behind the rest of the batch, meaning it's been
        // waiting much longer than the starvation threshold since it last
        // ran. It must be rescued to the front despite its exec_runtime.
        let tasks = vec![
            fake_task_with_stop_ts(1, 500, 100_000_000),
            fake_task_with_stop_ts(2, 10, 100_000_000),
            fake_task_with_stop_ts(3, 9999, 1),
        ];
        let ordered = order_tasks(Profile::Interactive, tasks);
        assert_eq!(pids(&ordered), vec![3, 2, 1]);
        assert_eq!(ordered.rescued, 1);
    }

    #[test]
    fn interactive_does_not_rescue_recently_run_tasks() {
        // Every stop_ts is close together (well under the threshold), so
        // this must behave exactly like plain exec_runtime sorting - the
        // starvation path should not fire just because tasks differ
        // slightly in stop_ts.
        let tasks = vec![
            fake_task_with_stop_ts(1, 500, 100_000_000),
            fake_task_with_stop_ts(2, 10, 99_000_000),
            fake_task_with_stop_ts(3, 9999, 100_000_000),
        ];
        let ordered = order_tasks(Profile::Interactive, tasks);
        assert_eq!(pids(&ordered), vec![2, 1, 3]);
        assert_eq!(ordered.rescued, 0);
    }

    #[test]
    fn never_run_tasks_are_not_counted_as_rescues() {
        // stop_ts == 0: brand new task. It still sorts first (zero
        // exec_runtime) but must not be reported as a starvation rescue.
        let tasks = vec![
            fake_task_with_stop_ts(1, 500, 100_000_000),
            fake_task_with_stop_ts(2, 0, 0),
        ];
        let ordered = order_tasks(Profile::Interactive, tasks);
        assert_eq!(pids(&ordered), vec![2, 1]);
        assert_eq!(ordered.rescued, 0);
    }

    #[test]
    fn performance_preserves_arrival_order() {
        let tasks = vec![fake_task(1, 500), fake_task(2, 10)];
        let ordered = order_tasks(Profile::Performance, tasks);
        assert_eq!(pids(&ordered), vec![1, 2]);
    }

    #[test]
    fn background_puts_short_bursts_ahead_of_hogs_in_arrival_order() {
        let tasks = vec![
            fake_task_with_stop_ts(1, 50_000_000, 100_000_000), // hog
            fake_task_with_stop_ts(2, 100_000, 100_000_000),    // short burst
            fake_task_with_stop_ts(3, 30_000_000, 100_000_000), // hog
            fake_task_with_stop_ts(4, 900_000, 100_000_000),    // short burst
        ];
        let ordered = order_tasks(Profile::Background, tasks);
        // Unlike Interactive, lanes are not sorted by exec_runtime inside:
        // pid 2 stays ahead of 4, and 1 ahead of 3.
        assert_eq!(pids(&ordered), vec![2, 4, 1, 3]);
        assert_eq!(ordered.rescued, 0);
    }

    #[test]
    fn background_rescues_a_starved_hog() {
        let tasks = vec![
            fake_task_with_stop_ts(1, 100_000, 300_000_000),
            fake_task_with_stop_ts(2, 50_000_000, 100_000_000), // waited 200ms
        ];
        let ordered = order_tasks(Profile::Background, tasks);
        assert_eq!(pids(&ordered), vec![2, 1]);
        assert_eq!(ordered.rescued, 1);
    }

    #[test]
    fn every_slice_is_non_zero() {
        let tasks = [
            fake_task(1, 0),
            fake_task(2, u64::MAX),
            fake_task_with_weight(0),
            fake_task_with_weight(10_000),
        ];
        let loads = [
            IDLE,
            Load {
                waiting: 10_000,
                nr_cpus: 1,
            },
            Load {
                waiting: 0,
                nr_cpus: 0,
            },
        ];
        for profile in [
            Profile::Balanced,
            Profile::Interactive,
            Profile::Performance,
            Profile::Background,
        ] {
            for task in &tasks {
                for load in loads {
                    assert!(slice_for(profile, task, load) > 0, "{profile:?}");
                }
            }
        }
    }

    #[test]
    fn balanced_slice_follows_weight_within_bounds() {
        let slice = |w| slice_for(Profile::Balanced, &fake_task_with_weight(w), IDLE);
        assert_eq!(slice(100), 5_000_000);
        assert_eq!(slice(200), 10_000_000);
        assert_eq!(slice(1), BALANCED_MIN_SLICE_NS);
        assert_eq!(slice(10_000), BALANCED_MAX_SLICE_NS);
    }

    #[test]
    fn interactive_slice_shrinks_with_queue_depth() {
        let task = fake_task(1, 0);
        let slice = |waiting| {
            slice_for(
                Profile::Interactive,
                &task,
                Load {
                    waiting,
                    nr_cpus: 4,
                },
            )
        };
        assert_eq!(slice(1), 2_000_000); // 1 per CPU
        assert_eq!(slice(8), 1_000_000); // 2 per CPU
        assert_eq!(slice(400), INTERACTIVE_MIN_SLICE_NS);
    }

    #[test]
    fn performance_slice_ignores_load_and_weight() {
        let busy = Load {
            waiting: 1000,
            nr_cpus: 1,
        };
        assert_eq!(
            slice_for(Profile::Performance, &fake_task_with_weight(1), busy),
            10_000_000
        );
    }

    #[test]
    fn background_gives_short_bursts_a_shorter_slice_than_hogs() {
        let burst = slice_for(Profile::Background, &fake_task(1, 100_000), IDLE);
        let hog = slice_for(Profile::Background, &fake_task(2, 50_000_000), IDLE);
        assert_eq!(burst, 5_000_000);
        assert_eq!(hog, 20_000_000);
    }
}
