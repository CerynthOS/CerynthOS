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

    /// Persists state to `path`, reporting rather than panicking on failure.
    ///
    /// This runs on every IPC request, so an unwritable state directory must
    /// not be able to take the daemon down.
    pub fn save(&self, path: &str) {
        if let Some(parent) = Path::new(path).parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                eprintln!(
                    "warning: cannot create state directory {}: {err}",
                    parent.display()
                );
                return;
            }
        }

        let json = match serde_json::to_string_pretty(self) {
            Ok(json) => json,
            Err(err) => {
                eprintln!("warning: cannot serialize runtime state: {err}");
                return;
            }
        };

        if let Err(err) = fs::write(path, json) {
            eprintln!("warning: cannot write state to {path}: {err}");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Each test gets its own path: the tests run concurrently and a shared
    /// filename makes them race against each other.
    fn test_file(name: &str) -> String {
        format!("target/test-state-{name}.json")
    }

    #[test]
    fn save_and_load_state() {
        let state = RuntimeState {
            profile: Profile::Interactive,
            adaptation_enabled: true,
            scheduler_backend: SchedulerBackend::Mock,
        };

        state.save(&test_file("roundtrip"));

        let loaded = RuntimeState::load(&test_file("roundtrip"));

        assert_eq!(loaded.profile, Profile::Interactive);
        assert!(loaded.adaptation_enabled);

        let _ = std::fs::remove_file(&test_file("roundtrip"));
    }

    #[test]
    fn missing_state_returns_default() {
        let _ = std::fs::remove_file(&test_file("missing"));

        let state = RuntimeState::load(&test_file("missing"));

        assert_eq!(state.profile, Profile::Balanced);
        assert!(!state.adaptation_enabled);
    }

    #[test]
    fn corrupt_state_returns_default() {
        std::fs::write(&test_file("corrupt"), "not json").unwrap();

        let state = RuntimeState::load(&test_file("corrupt"));

        assert_eq!(state.profile, Profile::Balanced);
        assert!(!state.adaptation_enabled);

        let _ = std::fs::remove_file(&test_file("corrupt"));
    }
}
