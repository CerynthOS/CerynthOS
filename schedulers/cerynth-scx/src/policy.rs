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