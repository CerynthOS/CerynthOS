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
}

impl SchedulerStatus {
    pub fn running(profile: Profile) -> Self {
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            running: true,
            pid: std::process::id(),
            profile: format!("{profile:?}"),
            healthy: true,
            started_at_ms,
        }
    }

    pub fn stopped() -> Self {
        Self {
            running: false,
            pid: std::process::id(),
            profile: String::new(),
            healthy: false,
            started_at_ms: 0,
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