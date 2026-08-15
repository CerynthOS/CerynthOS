use crate::bpf::QueuedTask;
use crate::profile::Profile;

/// A task waiting at least this long (in ns) since it last held a CPU jumps
/// to the front of the batch, ahead of the normal shortest-exec_runtime
/// ordering. Without this, a steady stream of short bursts under Interactive
/// could keep re-sorting a longer-running task to the back of every single
/// batch it appears in, indefinitely - never truly dropped, but never
/// serviced promptly either.
const STARVATION_THRESHOLD_NS: u64 = 20_000_000; // 20ms

/// Decide dispatch order for a batch of waiting tasks. Pure: no BPF, no
/// kernel calls - just a list in, a list out - which is what makes it
/// unit-testable without loading a real scheduler at all.

pub fn order_tasks(profile: Profile, mut tasks: Vec<QueuedTask>) -> Vec<QueuedTask> {
    match profile {
        Profile::Balanced => tasks, // FIFO: keep arrival order as-is
        Profile::Interactive => {
            rescue_starved_tasks(&mut tasks);
            tasks
        }
        Profile::Performance => tasks, // FIFO: fewer, longer slices do the work
        Profile::Background => tasks,  // FIFO: latency doesn't matter here
    }
}

/// Sorts the batch so starved tasks (waited too long since they last ran)
/// come first, ordered by how long they've waited; everything else keeps
/// the usual shortest-exec_runtime-first ordering.
///
/// `stop_ts` is when a task last released a CPU. Every task in one batch was
/// dequeued together, so the newest `stop_ts` in the batch stands in for
/// "now" - no wall-clock reading needed, which also avoids comparing against
/// a clock that isn't guaranteed to be in the same domain as these
/// kernel-side timestamps.
fn rescue_starved_tasks(tasks: &mut [QueuedTask]) {
    let Some(now) = tasks.iter().map(|task| task.stop_ts).max() else {
        return;
    };

    tasks.sort_by_key(|task| {
        let waited_ns = now.saturating_sub(task.stop_ts);
        if waited_ns >= STARVATION_THRESHOLD_NS {
            (0, u64::MAX - waited_ns)
        } else {
            (1, task.exec_runtime)
        }
    });
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

    #[test]
    fn balanced_preserves_arrival_order() {
        let tasks = vec![fake_task(1, 500), fake_task(2, 10), fake_task(3, 9999)];
        let ordered = order_tasks(Profile::Balanced, tasks);
        let pids: Vec<i32> = ordered.iter().map(|t| t.pid).collect();
        assert_eq!(pids, vec![1, 2, 3]);
    }
    #[test]
    fn interactive_sorts_by_exec_runtime_ascending() {
        let tasks = vec![fake_task(1, 500), fake_task(2, 10), fake_task(3, 9999)];
        let ordered = order_tasks(Profile::Interactive, tasks);
        let pids: Vec<i32> = ordered.iter().map(|t| t.pid).collect();
        assert_eq!(pids, vec![2, 1, 3]);
    }

    #[test]
    fn interactive_rescues_a_long_waiting_task() {
        // pid 3 has a huge exec_runtime (would normally sort last) but its
        // stop_ts is far behind the rest of the batch, meaning it's been
        // waiting much longer than STARVATION_THRESHOLD_NS since it last
        // ran. It must be rescued to the front despite its exec_runtime.
        let tasks = vec![
            fake_task_with_stop_ts(1, 500, 100_000_000),
            fake_task_with_stop_ts(2, 10, 100_000_000),
            fake_task_with_stop_ts(3, 9999, 0),
        ];
        let ordered = order_tasks(Profile::Interactive, tasks);
        let pids: Vec<i32> = ordered.iter().map(|t| t.pid).collect();
        assert_eq!(pids, vec![3, 2, 1]);
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
        let pids: Vec<i32> = ordered.iter().map(|t| t.pid).collect();
        assert_eq!(pids, vec![2, 1, 3]);
    }

    #[test]
    fn performance_and_background_preserve_arrival_order() {
        let tasks = vec![fake_task(1, 500), fake_task(2, 10)];

        let ordered = order_tasks(Profile::Performance, tasks.clone());
        assert_eq!(
            ordered.iter().map(|t| t.pid).collect::<Vec<_>>(),
            vec![1, 2]
        );

        let ordered = order_tasks(Profile::Background, tasks);
        assert_eq!(
            ordered.iter().map(|t| t.pid).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }
}
