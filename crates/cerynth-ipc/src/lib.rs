//! Core library module for this `CerynthOS` component.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Profile {
    Balanced,
    Interactive,
    Performance,
    Background,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchedulerBackend {
    Mock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerStatus {
    pub profile: Profile,
    pub adaptation_enabled: bool,
    pub backend: SchedulerBackend,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    Status,
    GetProfile,
    SetProfile(Profile),
    PauseAdaptation,
    ResumeAdaptation,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Status(SchedulerStatus),
    Profile(Profile),
    Success,
    Error(String),
}
