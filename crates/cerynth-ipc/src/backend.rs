use serde::{Deserialize, Serialize};

use crate::profile::Profile;

/// Represents the scheduler backend implementation in use.
///
/// Currently only a mock backend is available, but this enum is designed
/// to be extensible for future backends like SCX, BPF-based schedulers, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchedulerBackend {
    Mock,
}

impl SchedulerBackend {
    /// Returns all available backends.
    pub fn all() -> [SchedulerBackend; 1] {
        [SchedulerBackend::Mock]
    }
}

impl std::fmt::Display for SchedulerBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchedulerBackend::Mock => write!(f, "mock"),
        }
    }
}

impl std::str::FromStr for SchedulerBackend {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "mock" => Ok(SchedulerBackend::Mock),
            _ => Err(format!("unknown scheduler backend: {}", s)),
        }
    }
}

/// Status information returned by the scheduler daemon.
///
/// Contains the current profile, adaptation state, and active backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerStatus {
    /// The currently active performance profile.
    pub profile: Profile,

    /// Whether automatic profile adaptation is enabled.
    pub adaptation_enabled: bool,

    /// The scheduler backend currently in use.
    pub backend: SchedulerBackend,
}

impl SchedulerStatus {
    /// Creates a new SchedulerStatus with default values.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for SchedulerStatus {
    fn default() -> Self {
        Self {
            profile: Profile::Balanced,
            adaptation_enabled: false,
            backend: SchedulerBackend::Mock,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_backend_serialization() {
        let backend = SchedulerBackend::Mock;
        let json = serde_json::to_string(&backend).unwrap();
        assert_eq!(json, "\"mock\"");

        let backend: SchedulerBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(backend, SchedulerBackend::Mock);
    }

    #[test]
    fn scheduler_status_serialization() {
        let status = SchedulerStatus {
            profile: Profile::Performance,
            adaptation_enabled: true,
            backend: SchedulerBackend::Mock,
        };

        let json = serde_json::to_string(&status).unwrap();
        let parsed: SchedulerStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, parsed);
    }

    #[test]
    fn scheduler_status_default() {
        let status = SchedulerStatus::default();
        assert_eq!(status.profile, Profile::Balanced);
        assert!(!status.adaptation_enabled);
        assert_eq!(status.backend, SchedulerBackend::Mock);
    }
}