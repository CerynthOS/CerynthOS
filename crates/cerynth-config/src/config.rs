use cerynth_ipc::{Profile, SchedulerBackend};
use serde::{Deserialize, Serialize};

use std::fs;
use std::path::{Path, PathBuf};

/// Default path to the `cerynth-scx` binary, matching the install layout
/// produced by `scripts/install-dev-runtime.sh`.
pub const DEFAULT_SCHEDULER_BINARY: &str = "/usr/lib/cerynth/cerynth-scx";

fn default_scheduler_binary() -> PathBuf {
    PathBuf::from(DEFAULT_SCHEDULER_BINARY)
}

/// Helper so `auto_start` defaults to true when omitted from config.
fn default_true() -> bool {
    true
}

/// Persistent configuration loaded from a TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub default_profile: Profile,
    pub adaptation_enabled: bool,
    pub scheduler_backend: SchedulerBackend,

    /// Path to the scheduler binary the daemon launches.
    #[serde(default = "default_scheduler_binary")]
    pub scheduler_binary: PathBuf,

    /// Whether the daemon starts the scheduler automatically at boot.
    #[serde(default = "default_true")]
    pub auto_start: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_profile: Profile::Balanced,
            adaptation_enabled: false,
            scheduler_backend: SchedulerBackend::Mock,
            scheduler_binary: default_scheduler_binary(),
            auto_start: true,
        }
    }
}

impl Config {
    /// Loads configuration from `path`, falling back to defaults.
    ///
    /// A missing file is normal and silent. A file that exists but cannot be
    /// read or parsed is *not* normal: it is reported on stderr before the
    /// fallback, because a silently ignored config is far harder to debug
    /// than a broken one.
    pub fn load(path: &str) -> Self {
        if !Path::new(path).exists() {
            return Self::default();
        }

        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) => {
                eprintln!("warning: cannot read config {path}: {err}");
                eprintln!("warning: falling back to built-in defaults");
                return Self::default();
            }
        };

        match toml::from_str(&contents) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("warning: cannot parse config {path}: {err}");
                eprintln!("warning: falling back to built-in defaults");
                Self::default()
            }
        }
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

    /// Each test gets its own path: the tests run concurrently and a shared
    /// filename makes them race against each other.
    fn test_file(name: &str) -> String {
        format!("target/test-config-{name}.toml")
    }

    #[test]
    fn save_and_load_config() {
        const TEST_FILE: &str = "target/test-config-save-and-load.toml";

        let config = Config {
            default_profile: Profile::Performance,
            adaptation_enabled: true,
            scheduler_backend: SchedulerBackend::Mock,
            scheduler_binary: PathBuf::from("/usr/lib/cerynth/cerynth-scx"),
            auto_start: true,
        };

        config.save(&test_file("roundtrip"));

        let loaded = Config::load(&test_file("roundtrip"));

        assert_eq!(loaded.default_profile, Profile::Performance);
        assert!(loaded.adaptation_enabled);
        assert_eq!(loaded.scheduler_backend, SchedulerBackend::Mock);
        assert_eq!(
            loaded.scheduler_binary,
            PathBuf::from("/usr/lib/cerynth/cerynth-scx")
        );
        assert!(loaded.auto_start);

        let _ = std::fs::remove_file(&test_file("roundtrip"));
    }

    /// The config we actually ship must deserialize into `Config`.
    ///
    /// `load` falls back to defaults on a parse error, so without this test a
    /// malformed shipped config would be silently ignored at runtime.
    #[test]
    fn shipped_default_config_parses() {
        let shipped = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packaging/config/cerynth.toml"
        );

        let contents = std::fs::read_to_string(shipped)
            .unwrap_or_else(|e| panic!("cannot read {shipped}: {e}"));

        let config: Config = toml::from_str(&contents)
            .unwrap_or_else(|e| panic!("packaging/config/cerynth.toml does not parse: {e}"));

        assert_eq!(config.default_profile, Profile::Balanced);
        assert_eq!(config.scheduler_backend, SchedulerBackend::Scx);
        assert_eq!(
            config.scheduler_binary,
            std::path::PathBuf::from("/usr/lib/cerynth/cerynth-scx")
        );
        assert!(!config.adaptation_enabled);
    }

    #[test]
    fn missing_config_returns_default() {
        let _ = std::fs::remove_file(&test_file("missing"));

        let config = Config::load(&test_file("missing"));

        assert_eq!(config.default_profile, Profile::Balanced);
        assert!(!config.adaptation_enabled);
    }

    #[test]
    fn corrupt_config_returns_default() {
        if let Some(parent) = std::path::Path::new(&test_file("corrupt")).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        std::fs::write(&test_file("corrupt"), "this is not toml").unwrap();
        let config = Config::load(&test_file("corrupt"));

        assert_eq!(config.default_profile, Profile::Balanced);
        assert!(!config.adaptation_enabled);

        let _ = std::fs::remove_file(&test_file("corrupt"));
    }
}
