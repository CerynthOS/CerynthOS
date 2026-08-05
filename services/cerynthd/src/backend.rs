use std::sync::Arc;

use tokio::sync::Mutex;

use crate::state::DaemonState;
use cerynth_ipc::{Profile, SchedulerStatus};

/// Shared backend type used by the daemon.
pub type SharedBackend = Arc<Mutex<MockBackend>>;

/// Every scheduler backend (Mock, SCX, etc.) must implement this.
pub trait Backend {
    fn status(&self) -> SchedulerStatus;

    fn get_profile(&self) -> Profile;

    fn set_profile(&mut self, profile: Profile);

    fn pause_adaptation(&mut self);

    fn resume_adaptation(&mut self);
}

/// Temporary backend until the real SCX backend is implemented.
#[derive(Debug)]
pub struct MockBackend {
    state: DaemonState,
}

impl MockBackend {
    pub fn new(state: DaemonState) -> Self {
        Self { state }
    }
    pub fn state(&self) -> &DaemonState {
        &self.state
    }
}

impl Backend for MockBackend {
    fn status(&self) -> SchedulerStatus {
        SchedulerStatus {
            profile: self.state.profile.clone(),
            adaptation_enabled: self.state.adaptation_enabled,
            backend: self.state.scheduler_backend.clone(),
        }
    }

    fn get_profile(&self) -> Profile {
        self.state.profile.clone()
    }

    fn set_profile(&mut self, profile: Profile) {
        self.state.profile = profile;
    }

    fn pause_adaptation(&mut self) {
        self.state.adaptation_enabled = false;
    }

    fn resume_adaptation(&mut self) {
        self.state.adaptation_enabled = true;
    }
}
