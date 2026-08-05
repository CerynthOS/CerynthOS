use cerynth_ipc::{Profile, SchedulerBackend};
use serde::{Deserialize, Serialize};

use std::fs;
use std::path::Path;

/// Runtime state persisted by the daemon.
///
/// Unlike `Config`, this represents the current runtime state that
/// should survive daemon restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeState {
    pub profile: Profile,
    pub adaptation_enabled: bool,
    pub scheduler_backend: SchedulerBackend,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            profile: Profile::Balanced,
            adaptation_enabled: false,
            scheduler_backend: SchedulerBackend::Mock,
        }
    }
}

impl RuntimeState {
    pub fn load(path: &str) -> Self {
        if !Path::new(path).exists() {
            return Self::default();
        }

        let contents = fs::read_to_string(path).unwrap_or_default();

        serde_json::from_str(&contents).unwrap_or_default()
    }

    pub fn save(&self, path: &str) {
        if let Some(parent) = Path::new(path).parent() {
            let _ = fs::create_dir_all(parent);
        }

        let json = serde_json::to_string_pretty(self).unwrap();

        fs::write(path, json).unwrap();
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FILE: &str = "target/test-state.json";

    #[test]
    fn save_and_load_state() {
        let state = RuntimeState {
            profile: Profile::Interactive,
            adaptation_enabled: true,
            scheduler_backend: SchedulerBackend::Mock,
        };

        state.save(TEST_FILE);

        let loaded = RuntimeState::load(TEST_FILE);

        assert_eq!(loaded.profile, Profile::Interactive);
        assert!(loaded.adaptation_enabled);

        let _ = std::fs::remove_file(TEST_FILE);
    }

    #[test]
    fn missing_state_returns_default() {
        let _ = std::fs::remove_file(TEST_FILE);

        let state = RuntimeState::load(TEST_FILE);

        assert_eq!(state.profile, Profile::Balanced);
        assert!(!state.adaptation_enabled);
    }

    #[test]
    fn corrupt_state_returns_default() {
        std::fs::write(TEST_FILE, "not json").unwrap();

        let state = RuntimeState::load(TEST_FILE);

        assert_eq!(state.profile, Profile::Balanced);
        assert!(!state.adaptation_enabled);

        let _ = std::fs::remove_file(TEST_FILE);
    }
}
