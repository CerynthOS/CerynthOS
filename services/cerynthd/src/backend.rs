use std::sync::Arc;

use tokio::sync::Mutex;

use crate::backends::RollbackResult;
use cerynth_ipc::{AdaptationMode, Profile, SchedulerStatus};

/// Shared backend type used by the daemon.
pub type SharedBackend = Arc<Mutex<Box<dyn Backend + Send + Sync>>>;

/// Every scheduler backend (Mock, SCX, etc.) must implement this.
pub trait Backend {
    fn status(&mut self) -> Result<SchedulerStatus, String>;

    fn get_profile(&self) -> Result<Profile, String>;

    fn set_profile(&mut self, profile: Profile) -> Result<(), String>;

    fn get_adaptation_mode(&self) -> Result<AdaptationMode, String>;

    fn set_adaptation_mode(&mut self, mode: AdaptationMode) -> Result<(), String>;

    fn pause_adaptation(&mut self) -> Result<(), String>;

    fn resume_adaptation(&mut self) -> Result<(), String>;

    fn start(&mut self) -> Result<(), String>;

    fn stop(&mut self) -> Result<(), String>;

    fn restart(&mut self) -> Result<(), String>;

    /// Verifies the scheduler is healthy after a profile switch.
    ///
    /// Returns `Ok(())` if the scheduler is healthy (process alive, sched_ext active,
    /// heartbeat fresh, and profile matches). Returns `Err` with a reason if unhealthy.
    fn verify_post_switch_health(&mut self, expected_profile: Profile) -> Result<(), String>;

    /// Attempts a full rollback to a previous known-good profile.
    ///
    /// This stops the current scheduler, then attempts to start the previous good profile.
    /// Returns RollbackResult indicating success or failure.
    fn rollback(&mut self, previous_good_profile: Profile) -> crate::backends::RollbackResult;
}
