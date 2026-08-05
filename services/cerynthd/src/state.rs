use cerynth_config::RuntimeState;
use cerynth_ipc::{Profile, SchedulerBackend};

/// In-memory state owned by the daemon.
///
/// Unlike `RuntimeState`, this structure represents the daemon's
/// live state while it is running.
#[derive(Debug, Clone)]
pub struct DaemonState {
    pub profile: Profile,
    pub adaptation_enabled: bool,
    pub scheduler_backend: SchedulerBackend,
}

impl From<RuntimeState> for DaemonState {
    fn from(state: RuntimeState) -> Self {
        Self {
            profile: state.profile,
            adaptation_enabled: state.adaptation_enabled,
            scheduler_backend: state.scheduler_backend,
        }
    }
}
impl From<&DaemonState> for cerynth_config::RuntimeState {
    fn from(state: &DaemonState) -> Self {
        Self {
            profile: state.profile.clone(),
            adaptation_enabled: state.adaptation_enabled,
            scheduler_backend: state.scheduler_backend.clone(),
        }
    }
}
