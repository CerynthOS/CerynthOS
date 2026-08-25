use crate::backend::Backend;
use crate::backends::RollbackResult;
use crate::state::DaemonState;
use cerynth_ipc::{AdaptationMode, Profile, SchedulerStatus};

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
    fn status(&mut self) -> Result<SchedulerStatus, String> {
        Ok(SchedulerStatus {
            profile: self.state.profile.clone(),
            adaptation_enabled: self.state.adaptation_enabled,
            adaptation_mode: self.state.adaptation_mode,
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

    fn get_adaptation_mode(&self) -> Result<AdaptationMode, String> {
        Ok(self.state.adaptation_mode)
    }

    fn set_adaptation_mode(&mut self, mode: AdaptationMode) -> Result<(), String> {
        self.state.adaptation_mode = mode;
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

    fn verify_post_switch_health(&mut self, expected_profile: Profile) -> Result<(), String> {
        if self.state.profile == expected_profile {
            Ok(())
        } else {
            Err(format!(
                "post-switch health check failed: expected {:?}, got {:?}",
                expected_profile, self.state.profile
            ))
        }
    }

    fn rollback(&mut self, previous_good_profile: Profile) -> RollbackResult {
        // Mock: stop then try to set previous good profile
        let _ = self.stop();
        self.state.profile = previous_good_profile;
        RollbackResult::Success {
            previous_good_profile,
        }
    }
}
