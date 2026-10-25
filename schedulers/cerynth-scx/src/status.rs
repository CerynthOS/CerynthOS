use std::fs;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::profile::Profile;

const STATUS_DIR: &str = "/run/cerynth";
const STATUS_PATH: &str = "/run/cerynth/scheduler.json";
#[derive(Deserialize, Serialize)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_error: Option<String>,
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
            sched_ext_state: "enabled".to_string(),
            heartbeat: now_secs(),
            exit_error: None,
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
            exit_error: None,
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

/// Diagnostic counters only; they do not participate in scheduling decisions.
#[derive(Default, Deserialize, Serialize)]
pub struct SchedulerStats {
    pub pid: u32,
    pub profile: String,
    pub heartbeat: u64,
    pub scheduler_uptime_ms: u64,
    pub progress_sequence: u64,
    pub tasks_dequeued: u64,
    pub tasks_dispatched: u64,
    pub reporter_drops: u64,
    pub dispatch_errors: u64,
    pub dequeue_errors: u64,
    pub batches_processed: u64,
    pub avg_batch_size: f64,
    pub max_batch_size: u64,
    pub kernel_queued: u64,
    pub kernel_scheduled: u64,
    pub kernel_user_dispatches: u64,
    pub kernel_direct_dispatches: u64,
    pub kernel_failed_dispatches: u64,
    pub kernel_cancel_dispatches: u64,
}

impl SchedulerStats {
    pub fn write(&self) -> std::io::Result<()> {
        let path = format!("{STATUS_DIR}/scheduler-stats.json");
        let tmp = format!("{path}.tmp");
        fs::create_dir_all(STATUS_DIR)?;
        let json = serde_json::to_vec(self)?;
        fs::write(&tmp, json)?;
        fs::rename(tmp, path)
    }
}
