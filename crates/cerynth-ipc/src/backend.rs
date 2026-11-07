use serde::{Deserialize, Serialize};

use crate::profile::Profile;

/// Represents the scheduler backend implementation in use.
///
/// The mock backend is used when no real scheduler is available; the SCX
/// backend drives the sched_ext-based `cerynth-scx` scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchedulerBackend {
    /// In-process stand-in used by tests and by the daemon before the real
    /// backend is wired up.
    Mock,
    /// Drives a real `cerynth-scx` child process via sched_ext.
    Scx,
}

impl SchedulerBackend {
    /// Returns all available backends.
    pub fn all() -> [SchedulerBackend; 2] {
        [SchedulerBackend::Mock, SchedulerBackend::Scx]
    }
}

impl std::fmt::Display for SchedulerBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchedulerBackend::Mock => write!(f, "mock"),
            SchedulerBackend::Scx => write!(f, "scx"),
        }
    }
}

impl std::str::FromStr for SchedulerBackend {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "mock" => Ok(SchedulerBackend::Mock),
            "scx" => Ok(SchedulerBackend::Scx),
            _ => Err(format!("unknown scheduler backend: {}", s)),
        }
    }
}

/// Adaptation mode controlling whether and how the daemon applies policy recommendations.
///
/// - `Off`: No adaptation; the daemon never reads or acts on policy.
/// - `Shadow`: Reads policy and logs recommendations but never switches the scheduler.
/// - `Canary`: Reads policy, validates the recommendation, and applies a guarded switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdaptationMode {
    Off,
    Shadow,
    Canary,
}

impl AdaptationMode {
    /// Returns all available adaptation modes in declaration order.
    pub fn all() -> [AdaptationMode; 3] {
        [
            AdaptationMode::Off,
            AdaptationMode::Shadow,
            AdaptationMode::Canary,
        ]
    }
}

impl std::fmt::Display for AdaptationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdaptationMode::Off => write!(f, "off"),
            AdaptationMode::Shadow => write!(f, "shadow"),
            AdaptationMode::Canary => write!(f, "canary"),
        }
    }
}

impl std::str::FromStr for AdaptationMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" => Ok(AdaptationMode::Off),
            "shadow" => Ok(AdaptationMode::Shadow),
            "canary" => Ok(AdaptationMode::Canary),
            _ => Err(format!("unknown adaptation mode: {}", s)),
        }
    }
}

/// Status information returned by the scheduler daemon.
///
/// Contains the current profile, adaptation state, active backend, and
/// health signals for the scheduler: whether a process is running, what
/// the sched_ext subsystem reports, and whether the scheduler heartbeat
/// is fresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerStatus {
    /// The currently active performance profile.
    pub profile: Profile,

    /// Whether automatic profile adaptation is enabled.
    pub adaptation_enabled: bool,

    /// The adaptation mode controlling how recommendations are applied.
    pub adaptation_mode: AdaptationMode,

    /// The scheduler backend currently in use.
    pub backend: SchedulerBackend,

    /// Whether a scheduler process is currently running.
    #[serde(default)]
    pub running: bool,

    /// Name of the scheduler currently loaded in the sched_ext subsystem
    /// (e.g. "scx_rustland"), or `None` when sched_ext is unavailable.
    #[serde(default)]
    pub sched_ext_state: Option<String>,

    /// Whether the scheduler's heartbeat file is present and fresh.
    #[serde(default)]
    pub heartbeat_ok: bool,
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
            adaptation_mode: AdaptationMode::Off,
            backend: SchedulerBackend::Mock,
            running: false,
            sched_ext_state: None,
            heartbeat_ok: false,
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
            adaptation_mode: AdaptationMode::Shadow,
            backend: SchedulerBackend::Mock,
            running: true,
            sched_ext_state: Some("scx_rustland".to_string()),
            heartbeat_ok: true,
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
        assert_eq!(status.adaptation_mode, AdaptationMode::Off);
        assert_eq!(status.backend, SchedulerBackend::Mock);
        assert!(!status.running);
        assert_eq!(status.sched_ext_state, None);
        assert!(!status.heartbeat_ok);
    }

    #[test]
    fn scx_backend_serialization() {
        let backend = SchedulerBackend::Scx;
        let json = serde_json::to_string(&backend).unwrap();
        assert_eq!(json, "\"scx\"");

        let parsed: SchedulerBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SchedulerBackend::Scx);

        assert_eq!(SchedulerBackend::Scx.to_string(), "scx");
        assert!(matches!(
            "scx".parse::<SchedulerBackend>(),
            Ok(SchedulerBackend::Scx)
        ));
    }

    #[test]
    fn adaptation_mode_serialization() {
        let mode = AdaptationMode::Off;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"off\"");

        let mode: AdaptationMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, AdaptationMode::Off);

        assert_eq!(AdaptationMode::Off.to_string(), "off");
        assert!(matches!(
            "off".parse::<AdaptationMode>(),
            Ok(AdaptationMode::Off)
        ));

        let mode = AdaptationMode::Shadow;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"shadow\"");

        let mode: AdaptationMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, AdaptationMode::Shadow);

        let mode = AdaptationMode::Canary;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"canary\"");

        let mode: AdaptationMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, AdaptationMode::Canary);
    }

    #[test]
    fn invalid_adaptation_mode_rejected() {
        assert!("invalid".parse::<AdaptationMode>().is_err());
        assert!("offf".parse::<AdaptationMode>().is_err());
        assert!("canaryy".parse::<AdaptationMode>().is_err());
    }
}
