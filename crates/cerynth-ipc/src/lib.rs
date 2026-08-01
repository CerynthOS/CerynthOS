//! Core library module for this `CerynthOS` component.
#[derive(Debug, Clone)]
pub enum Profile {
    Balanced,
    Interactive,
    Performance,
    Background,
}

#[derive(Debug, Clone)]
pub enum SchedulerBackend {
    Mock,
}

#[derive(Debug, Clone)]
pub struct SchedulerStatus {
    pub profile: Profile,
    pub adaptation_enabled: bool,
    pub backend: SchedulerBackend,
}

#[derive(Debug)]
pub enum Request {
    Status,
    GetProfile,
    SetProfile(Profile),
    PauseAdaptation,
    ResumeAdaptation,
}

#[derive(Debug)]
pub enum Response {
    Status(SchedulerStatus),
    Profile(Profile),
    Success,
    Error(String),
}
