use std::sync::Arc;

use tokio::sync::Mutex;

use cerynth_ipc::{Profile, SchedulerStatus};

/// Shared backend type used by the daemon.
pub type SharedBackend = Arc<Mutex<Box<dyn Backend + Send + Sync>>>;

/// Every scheduler backend (Mock, SCX, etc.) must implement this.
pub trait Backend {
    fn status(&mut self) -> Result<SchedulerStatus, String>;

    fn get_profile(&self) -> Result<Profile, String>;

    fn set_profile(&mut self, profile: Profile) -> Result<(), String>;

    fn pause_adaptation(&mut self) -> Result<(), String>;

    fn resume_adaptation(&mut self) -> Result<(), String>;

    fn start(&mut self) -> Result<(), String>;

    fn stop(&mut self) -> Result<(), String>;

    fn restart(&mut self) -> Result<(), String>;
}
