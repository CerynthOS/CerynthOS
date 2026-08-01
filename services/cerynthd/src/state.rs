use cerynth_ipc::{Profile, SchedulerBackend};

#[derive(Debug, Clone)]
pub struct DaemonState {
    pub profile: Profile,
    pub adaptation_enabled: bool,
    pub scheduler_backend: SchedulerBackend,
}
