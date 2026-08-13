//! Runtime paths used by the daemon.
//!
//! These paths may be overridden through environment variables so the daemon
//! can run unprivileged during development and integration tests.

use std::sync::OnceLock;

/// Default configuration file.
pub const DEFAULT_CONFIG_PATH: &str = "/etc/cerynth/cerynth.toml";

/// Default persistent state file.
pub const DEFAULT_STATE_PATH: &str = "/var/lib/cerynth/state.json";

fn resolve(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.to_string())
}

/// Path to the configuration file.
///
/// `CERYNTH_CONFIG` overrides the system default.
pub fn config_path() -> &'static str {
    static CONFIG_PATH: OnceLock<String> = OnceLock::new();

    CONFIG_PATH.get_or_init(|| resolve("CERYNTH_CONFIG", DEFAULT_CONFIG_PATH))
}

/// Path to the persistent runtime state file.
///
/// `CERYNTH_STATE` overrides the system default.
pub fn state_path() -> &'static str {
    static STATE_PATH: OnceLock<String> = OnceLock::new();

    STATE_PATH.get_or_init(|| resolve("CERYNTH_STATE", DEFAULT_STATE_PATH))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_runtime_contract() {
        assert_eq!(DEFAULT_CONFIG_PATH, "/etc/cerynth/cerynth.toml");
        assert_eq!(DEFAULT_STATE_PATH, "/var/lib/cerynth/state.json");
    }

    #[test]
    fn paths_are_stable_across_calls() {
        assert_eq!(config_path(), config_path());
        assert_eq!(state_path(), state_path());
    }
}
