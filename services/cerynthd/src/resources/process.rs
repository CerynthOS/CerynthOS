//! Process information from `/proc`.
//!
//! Reads kernel-maintained process identity and status without trusting agent
//! self-reports. Missing or exited processes yield `None` rather than zero.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Process identity and status from `/proc/<pid>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: i32,
    pub ppid: Option<i32>,
    pub process_group: Option<i32>,
    pub session_id: Option<i32>,
    pub name: Option<String>,
    pub state: Option<String>,
    /// Resident set size in pages (from `/proc/<pid>/statm`).
    pub rss_pages: Option<u64>,
}

impl ProcessInfo {
    /// Read information for a PID from `/proc`.
    pub fn read(pid: i32) -> Result<Option<Self>> {
        Self::read_from(Path::new("/proc"), pid)
    }

    /// Read from an explicit proc root (for tests/fixtures).
    pub fn read_from(proc_root: &Path, pid: i32) -> Result<Option<Self>> {
        let dir = proc_root.join(pid.to_string());
        if !dir.exists() {
            return Ok(None);
        }

        let status = std::fs::read_to_string(dir.join("status"))
            .with_context(|| format!("read {}", dir.join("status").display()))?;
        let mut parent_pid = None;
        let mut process_group = None;
        let mut session_id = None;
        let mut name = None;
        let mut state = None;
        for line in status.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key {
                "Name" => name = Some(value.to_string()),
                "State" => state = value.split_whitespace().next().map(str::to_string),
                "PPid" => parent_pid = value.parse().ok(),
                _ => {}
            }
        }

        // /proc/<pid>/stat: pid (comm) state ppid pgrp session ...
        // comm can contain spaces and parentheses, so find the last ')'.
        if let Ok(stat) = std::fs::read_to_string(dir.join("stat")) {
            if let Some(close) = stat.rfind(')') {
                let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
                // After ')' the fields are: state(3), ppid(4), pgrp(5), session(6).
                if fields.len() > 3 {
                    process_group = fields[2].parse().ok();
                    session_id = fields[3].parse().ok();
                }
            }
        }

        let rss_pages = std::fs::read_to_string(dir.join("statm"))
            .ok()
            .and_then(|s| s.split_whitespace().nth(1).and_then(|v| v.parse().ok()));

        Ok(Some(Self {
            pid,
            ppid: parent_pid,
            process_group,
            session_id,
            name,
            state,
            rss_pages,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_process_fixture() {
        let dir = std::env::temp_dir().join(format!("proc-test-{}", uuid::Uuid::new_v4()));
        let pid_dir = dir.join("4242");
        std::fs::create_dir_all(&pid_dir).unwrap();
        std::fs::write(
            pid_dir.join("status"),
            "Name:\tworker\nState:\tS (sleeping)\nPid:\t4242\nPPid:\t1\n",
        )
        .unwrap();
        std::fs::write(
            pid_dir.join("stat"),
            "4242 (worker) S 1 4242 4242 0 -1 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
        )
        .unwrap();
        std::fs::write(pid_dir.join("statm"), "100 42 0 0 0 0 0\n").unwrap();

        let info = ProcessInfo::read_from(&dir, 4242).unwrap().unwrap();
        assert_eq!(info.pid, 4242);
        assert_eq!(info.ppid, Some(1));
        assert_eq!(info.process_group, Some(4242));
        assert_eq!(info.name.as_deref(), Some("worker"));
        assert_eq!(info.rss_pages, Some(42));

        assert!(ProcessInfo::read_from(&dir, 9999).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
