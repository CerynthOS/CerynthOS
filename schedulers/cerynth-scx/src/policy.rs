use crate::bpf::QueuedTask;
use crate::profile::Profile;

/// Decide dispatch order for a batch of waiting tasks. Pure: no BPF, no
/// kernel calls - just a list in, a list out - which is what makes it
/// unit-testable without loading a real scheduler at all.

pub fn order_tasks(profile: Profile, mut tasks: Vec<QueuedTask>) -> Vec<QueuedTask> {
    match profile {
        Profile::Balanced => tasks, // FIFO: keep arrival order as-is
        Profile::Interactive => {
            tasks.sort_by_key(|task| task.exec_runtime);
            tasks
        }
        Profile::Performance => tasks,  // FIFO: fewer, longer slices do the work
        Profile::Background => tasks,  // FIFO: latency doesn't matter here
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_task(pid: i32, exec_runtime: u64) -> QueuedTask {
        QueuedTask {
            pid,
            cpu: 0,
            nr_cpus_allowed: 1,
            flags: 0,
            start_ts: 0,
            stop_ts: 0,
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
    fn performance_and_background_preserve_arrival_order() {
        let tasks = vec![fake_task(1, 500), fake_task(2, 10)];

        let ordered = order_tasks(Profile::Performance, tasks.clone());
        assert_eq!(ordered.iter().map(|t| t.pid).collect::<Vec<_>>(), vec![1, 2]);

        let ordered = order_tasks(Profile::Background, tasks);
        assert_eq!(ordered.iter().map(|t| t.pid).collect::<Vec<_>>(), vec![1, 2]);
    }
}