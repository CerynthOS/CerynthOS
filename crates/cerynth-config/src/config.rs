use cerynth_ipc::{Profile, SchedulerBackend};
use serde::{Deserialize, Serialize};

use std::fs;
use std::path::Path;

/// Persistent configuration loaded from a TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub default_profile: Profile,
    pub adaptation_enabled: bool,
    pub scheduler_backend: SchedulerBackend,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_profile: Profile::Balanced,
            adaptation_enabled: false,
            scheduler_backend: SchedulerBackend::Mock,
        }
    }
}

impl Config {
    pub fn load(path: &str) -> Self {
        if !Path::new(path).exists() {
            return Self::default();
        }

        let contents = fs::read_to_string(path).unwrap_or_default();

        toml::from_str(&contents).unwrap_or_default()
    }

    pub fn save(&self, path: &str) {
        if let Some(parent) = Path::new(path).parent() {
            let _ = fs::create_dir_all(parent);
        }

        let contents = toml::to_string_pretty(self).unwrap();

        fs::write(path, contents).unwrap();
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FILE: &str = "target/test-config.toml";

    #[test]
    fn save_and_load_config() {
        let config = Config {
            default_profile: Profile::Performance,
            adaptation_enabled: true,
            scheduler_backend: SchedulerBackend::Mock,
        };

        config.save(TEST_FILE);

        let loaded = Config::load(TEST_FILE);

        assert_eq!(loaded.default_profile, Profile::Performance);
        assert!(loaded.adaptation_enabled);
        assert_eq!(loaded.scheduler_backend, SchedulerBackend::Mock);

        let _ = std::fs::remove_file(TEST_FILE);
    }

    #[test]
    fn missing_config_returns_default() {
        let _ = std::fs::remove_file(TEST_FILE);

        let config = Config::load(TEST_FILE);

        assert_eq!(config.default_profile, Profile::Balanced);
        assert!(!config.adaptation_enabled);
    }

    #[test]
    fn corrupt_config_returns_default() {
    	if let Some(parent) = std::path::Path::new(TEST_FILE).parent() {
    	    let _ = std::fs::create_dir_all(parent);
	}

        std::fs::write(TEST_FILE, "this is not toml").unwrap();
        let config = Config::load(TEST_FILE);

        assert_eq!(config.default_profile, Profile::Balanced);
        assert!(!config.adaptation_enabled);

        let _ = std::fs::remove_file(TEST_FILE);
    }
}
