use serde::{Deserialize, Serialize};

use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeState {
    pub profile: String,
    pub adaptation_enabled: bool,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            profile: "balanced".into(),
            adaptation_enabled: false,
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
