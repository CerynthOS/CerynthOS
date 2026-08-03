use serde::{Deserialize, Serialize};

use std::fs;
use std::path::Path;
pub mod state;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CerynthConfig {
    pub default_profile: String,
    pub adaptation_enabled: bool,
    pub scheduler_backend: String,
}

impl Default for CerynthConfig {
    fn default() -> Self {
        Self {
            default_profile: "balanced".to_string(),
            adaptation_enabled: false,
            scheduler_backend: "mock".to_string(),
        }
    }
}

impl CerynthConfig {

    pub fn load(path: &str) -> Self {

        if !Path::new(path).exists() {
            return Self::default();
        }

        let contents =
            fs::read_to_string(path)
                .unwrap_or_default();

        toml::from_str(&contents)
            .unwrap_or_default()
    }

    pub fn save(&self, path: &str) {

        if let Some(parent) = Path::new(path).parent() {
            let _ = fs::create_dir_all(parent);
        }

        let contents =
            toml::to_string_pretty(self).unwrap();

        fs::write(path, contents).unwrap();
    }
}
