//! Runtime paths used by the daemon.
//!
//! These are frozen by `docs/contracts/runtime-v1.md`. They live in one module
//! and are resolved exactly once, because the daemon previously loaded its
//! state from one hardcoded path and saved it to a different one.
//!
//! Each path may be overridden by an environment variable so the daemon can
//! run unprivileged during development and tests.

use std::sync::OnceLock;

/// Default configuration file. Override with `CERYNTH_CONFIG`.
pub const DEFAULT_CONFIG_PATH: &str = "/etc/cerynth/cerynth.toml";

/// Default persistent state file. Override with `CERYNTH_STATE`.
pub const DEFAULT_STATE_PATH: &str = "/var/lib/cerynth/state.json";

fn resolve(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.to_string())
}

/// Path to the configuration file.
pub fn config_path() -> &'static str {
    static CONFIG_PATH: OnceLock<String> = OnceLock::new();

    CONFIG_PATH.get_or_init(|| resolve("CERYNTH_CONFIG", DEFAULT_CONFIG_PATH))
}

/// Path to the persistent runtime state file.
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
