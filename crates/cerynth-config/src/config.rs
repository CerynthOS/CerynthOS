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
