use std::fs;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::profile::Profile;

const STATUS_DIR: &str = "/run/cerynth";
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
    /// written. cerynthd's ScxBackend reads this field (named exactly
    /// "heartbeat", in seconds) to decide whether the scheduler is still
    /// alive: it considers the scheduler unhealthy once this is more than
    /// HEARTBEAT_TIMEOUT_SECS old. main.rs's run loop refreshes this
    /// periodically while dispatching, not just at start/stop.
    pub heartbeat: u64,
}
fn read_sched_ext_state() -> String {
    fs::read_to_string("/sys/kernel/sched_ext/state")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unavailable".to_string())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl SchedulerStatus {
    /// `started_at_ms` is the scheduler's real start time, captured once by
    /// the caller. Passing it in (instead of recomputing it here) is what
    /// lets `run()` call this repeatedly to refresh only `heartbeat`
    /// without started_at_ms drifting forward on every refresh.
    pub fn running(profile: Profile, started_at_ms: u64) -> Self {
        Self {
            running: true,
            pid: std::process::id(),
            profile: format!("{profile:?}"),
            healthy: true,
            started_at_ms,
            sched_ext_state: read_sched_ext_state(),
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
        fs::create_dir_all(STATUS_DIR)?;
        let tmp_path = format!("{STATUS_PATH}.tmp");
        let json = serde_json::to_string_pretty(self).unwrap_or_default();
        {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, STATUS_PATH)?;
        Ok(())
    }
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
}
