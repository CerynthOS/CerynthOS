use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::profile::Profile;

const STATUS_PATH: &str = "/run/cerynth/scheduler.json";

#[derive(Serialize)]
pub struct SchedulerStatus {
    pub running: bool,
    pub pid: u32,
    pub profile: String,
    pub healthy: bool,
    pub started_at_ms: u64,
    pub sched_ext_state: String,

    /// Unix timestamp (whole seconds) of the last time this status was
    /// written. cerynthd's `ScxBackend` reads this field (named exactly
    /// "heartbeat", in seconds) to decide whether the scheduler is still
    /// alive: it considers the scheduler unhealthy once this is more than
    /// `HEARTBEAT_TIMEOUT_SECS` old. main.rs's run loop refreshes this
    /// periodically while dispatching, not just at start/stop.
    pub heartbeat: u64,
}

pub fn read_sched_ext_state() -> String {
    fs::read_to_string("/sys/kernel/sched_ext/state")
        .map_or_else(|_| "unavailable".to_string(), |s| s.trim().to_string())
}

pub fn now_ms() -> u64 {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u64::try_from(ms).unwrap_or(u64::MAX)
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl SchedulerStatus {
    /// `started_at_ms` is the scheduler's real start time, captured once by
    /// the caller. Passing it in (instead of recomputing it here) is what
    /// lets `run()` call this repeatedly to refresh only `heartbeat`
    /// without `started_at_ms` drifting forward on every refresh.
    ///
    /// `healthy` is only true while the kernel agrees a BPF scheduler is
    /// loaded: a live process whose scheduler the kernel already kicked out
    /// (watchdog, `SysRq-S`) is running but not healthy.
    pub fn running(profile: Profile, started_at_ms: u64) -> Self {
        let sched_ext_state = read_sched_ext_state();
        Self {
            running: true,
            pid: std::process::id(),
            profile: profile.as_str().to_string(),
            healthy: sched_ext_state == "enabled",
            started_at_ms,
            sched_ext_state,
            heartbeat: now_secs(),
        }
    }

    pub fn stopped() -> Self {
        Self {
            running: false,
            pid: std::process::id(),
            profile: String::new(),
            healthy: false,
            started_at_ms: 0,
            sched_ext_state: read_sched_ext_state(),
            heartbeat: now_secs(),
        }
    }

    pub fn write(&self) -> std::io::Result<()> {
        write_json_atomic(Path::new(STATUS_PATH), self)
    }
}

/// Write `value` as JSON so readers only ever see the old file or the new
/// one, never a half-written mix: write a temp file next to it, flush it to
/// disk, then rename it over the target (a rename within one directory is
/// atomic). Recreates the parent directory if something deleted it.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let mut tmp_path = path.as_os_str().to_owned();
    tmp_path.push(".tmp");
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // cerynthd's ScxBackend reads this exact field name/shape:
    // json.get("heartbeat").and_then(serde_json::Value::as_u64), interpreted
    // as whole seconds since the epoch. This test exists so a future
    // rename/retype of `heartbeat` fails loudly here instead of silently at
    // runtime in cerynthd's health check.
    #[test]
    fn heartbeat_field_matches_cerynthd_contract() {
        let status = SchedulerStatus::running(Profile::Balanced, 1_000);
        let json: serde_json::Value = serde_json::to_value(&status).unwrap();

        let heartbeat = json.get("heartbeat").and_then(serde_json::Value::as_u64);
        assert!(heartbeat.is_some(), "heartbeat must serialize as a u64");

        let now = now_secs();
        assert!(heartbeat.unwrap() <= now && heartbeat.unwrap() + 2 >= now);
    }

    #[test]
    fn profile_is_serialized_lowercase() {
        let status = SchedulerStatus::running(Profile::Interactive, 1_000);
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["profile"], "interactive");
    }

    #[test]
    fn atomic_write_recreates_a_deleted_directory_and_replaces_garbage() {
        let dir = std::env::temp_dir().join(format!("cerynth-scx-status-{}", std::process::id()));
        let path = dir.join("scheduler.json");

        // Directory missing entirely (someone ran rm -rf /run/cerynth).
        let _ = fs::remove_dir_all(&dir);
        write_json_atomic(&path, &serde_json::json!({ "a": 1 })).unwrap();

        // File corrupted by something else: the next write must replace it.
        fs::write(&path, "{ not json").unwrap();
        write_json_atomic(&path, &serde_json::json!({ "a": 2 })).unwrap();

        let back: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back["a"], 2);
        assert!(!dir.join("scheduler.json.tmp").exists());

        fs::remove_dir_all(&dir).unwrap();
    }
}
