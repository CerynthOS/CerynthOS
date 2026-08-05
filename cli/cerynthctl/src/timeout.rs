use std::time::Duration;

/// Default timeout used by the CLI when communicating with the daemon.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
