use crate::backend::Backend;
use crate::state::DaemonState;
use cerynth_ipc::{Profile, SchedulerStatus};

/// Temporary backend until the real SCX backend is implemented.
#[derive(Debug)]
pub struct MockBackend {
    state: DaemonState,
}

impl MockBackend {
    pub fn new(state: DaemonState) -> Self {
        Self { state }
    }
}

impl Backend for MockBackend {
    fn status(&self) -> Result<SchedulerStatus, String> {
        Ok(SchedulerStatus {
            profile: self.state.profile.clone(),
            adaptation_enabled: self.state.adaptation_enabled,
            backend: self.state.scheduler_backend.clone(),
            running: true,
            // The mock reports no real subsystem or heartbeat signals.
            sched_ext_state: None,
            heartbeat_ok: false,
        })
    }

    fn get_profile(&self) -> Result<Profile, String> {
        Ok(self.state.profile.clone())
    }

    fn set_profile(&mut self, profile: Profile) -> Result<(), String> {
        self.state.profile = profile;
        Ok(())
    }

    fn pause_adaptation(&mut self) -> Result<(), String> {
        self.state.adaptation_enabled = false;
        Ok(())
    }

    fn resume_adaptation(&mut self) -> Result<(), String> {
        self.state.adaptation_enabled = true;
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn restart(&mut self) -> Result<(), String> {
        Ok(())
    }
}
