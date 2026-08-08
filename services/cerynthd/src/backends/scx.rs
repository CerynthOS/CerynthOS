use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cerynth_ipc::{Profile, SchedulerBackend, SchedulerStatus};

use crate::backend::Backend;

/// How old a heartbeat timestamp may be before the scheduler is considered
/// unhealthy.
const HEARTBEAT_TIMEOUT_SECS: u64 = 10;

/// Default sched_ext subsystem state file.
///
/// Contains the name of the currently loaded sched_ext scheduler (e.g.
/// "scx_rustland") or "none" when idle. Override with `CERYNTH_SCX_STATE_PATH`
/// for testing.
const DEFAULT_SCX_STATE_PATH: &str = "/sys/kernel/sched_ext/state";

/// Default scheduler heartbeat file.
///
/// Written periodically by the scheduler process as JSON:
/// `{ "pid": <u32>, "profile": "<name>", "heartbeat": <unix_secs> }`.
/// Override with `CERYNTH_HEARTBEAT_PATH` for testing.
const DEFAULT_HEARTBEAT_PATH: &str = "/run/cerynth/scheduler.json";

/// Path to the sched_ext state file, overridable for testing.
fn scx_state_path() -> PathBuf {
    std::env::var_os("CERYNTH_SCX_STATE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SCX_STATE_PATH))
}

/// Path to the scheduler heartbeat file, overridable for testing.
fn heartbeat_path() -> PathBuf {
    std::env::var_os("CERYNTH_HEARTBEAT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_HEARTBEAT_PATH))
}

/// Reads the name of the scheduler currently loaded in sched_ext, or `None`
/// when the subsystem is unavailable (e.g. a non-sched_ext kernel).
fn read_scx_state_from(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let state = contents.trim();
    if state.is_empty() {
        None
    } else {
        Some(state.to_string())
    }
}

/// Returns true when the heartbeat file exists, parses as the documented
/// JSON contract, and contains a `heartbeat` timestamp fresh enough.
fn heartbeat_is_fresh(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return false;
    };
    let Some(timestamp) = json.get("heartbeat").and_then(serde_json::Value::as_u64) else {
        return false;
    };
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return false;
    };
    now.as_secs().saturating_sub(timestamp) <= HEARTBEAT_TIMEOUT_SECS
}

pub struct ScxBackend {
    scheduler_binary: PathBuf,
    profile: Profile,
    adaptation_enabled: bool,
    pid: Option<u32>,
    child: Option<Child>,
}

impl ScxBackend {
    pub fn new(scheduler_binary: PathBuf, profile: Profile) -> Self {
        Self {
            scheduler_binary,
            profile,
            adaptation_enabled: true,
            pid: None,
            child: None,
        }
    }

    /// Overrides the initial adaptation state, e.g. from the daemon's
    /// persisted runtime state rather than always defaulting to enabled.
    pub fn with_adaptation(mut self, adaptation_enabled: bool) -> Self {
        self.adaptation_enabled = adaptation_enabled;
        self
    }
}

impl Backend for ScxBackend {
    fn status(&self) -> Result<SchedulerStatus, String> {
        // Probe for a live process by sending signal 0: kill(pid, 0) returns
        // 0 when a process with this PID exists, -1 otherwise (e.g. ESRCH for
        // a dead or recycled PID).
        let running = self.pid.is_some_and(|pid| {
            // Signal 0 probes for existence: returns 0 if the PID is alive.
            unsafe { libc::kill(pid as i32, 0) == 0 }
        });

        // A scheduler that is not running cannot be producing a fresh
        // heartbeat, so only check freshness while the process is alive.
        let heartbeat_ok = running && heartbeat_is_fresh(&heartbeat_path());

        Ok(SchedulerStatus {
            profile: self.profile.clone(),
            adaptation_enabled: self.adaptation_enabled,
            backend: SchedulerBackend::Scx,
            running,
            sched_ext_state: read_scx_state_from(&scx_state_path()),
            heartbeat_ok,
        })
    }

    fn get_profile(&self) -> Result<Profile, String> {
        Ok(self.profile.clone())
    }

    fn set_profile(&mut self, profile: Profile) -> Result<(), String> {
        // If a scheduler is running, restart it so the new profile takes
        // effect. If not running, just update the stored profile.
        if self.pid.is_some() {
            self.stop()?;
            self.profile = profile;
            self.start()?;
        } else {
            self.profile = profile;
        }
        Ok(())
    }

    fn pause_adaptation(&mut self) -> Result<(), String> {
        self.adaptation_enabled = false;
        Ok(())
    }

    fn resume_adaptation(&mut self) -> Result<(), String> {
        self.adaptation_enabled = true;
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        // If a scheduler process is already tracked, refuse to double-start.
        if self.pid.is_some() {
            return Err("scheduler is already running".to_string());
        }

        // The scheduler binary must exist before we try to launch it.
        if !self.scheduler_binary.exists() {
            return Err(format!(
                "scheduler binary not found: {}",
                self.scheduler_binary.display()
            ));
        }

        // Launch cerynth-scx with the currently selected profile.
        let child = Command::new(&self.scheduler_binary)
            .arg("--profile")
            .arg(self.profile.to_string())
            .spawn()
            .map_err(|e| format!("failed to spawn scheduler: {e}"))?;

        self.pid = Some(child.id());
        self.child = Some(child);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        // There must be a tracked scheduler to stop.
        let Some(child) = self.child.as_mut() else {
            return Err("no scheduler is currently running".to_string());
        };
        let pid = self
            .pid
            .ok_or_else(|| "no scheduler is currently running".to_string())?;

        // Request a graceful shutdown via SIGTERM.
        if unsafe { libc::kill(pid as i32, libc::SIGTERM) != 0 } {
            return Err(format!(
                "failed to send SIGTERM to scheduler (pid {pid}): {}",
                std::io::Error::last_os_error()
            ));
        }

        // Give the process a bounded grace period to exit after SIGTERM.
        // If it does not exit within the timeout, escalate to SIGKILL.
        let poll_interval = Duration::from_millis(100);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break, // exited gracefully
                Ok(None) => {
                    if Instant::now() >= deadline {
                        // Grace period elapsed: force termination.
                        if unsafe { libc::kill(pid as i32, libc::SIGKILL) != 0 } {
                            return Err(format!(
                                "failed to send SIGKILL to scheduler (pid {pid}): {}",
                                std::io::Error::last_os_error()
                            ));
                        }
                        child
                            .wait()
                            .map_err(|e| format!("failed to reap scheduler after SIGKILL: {e}"))?;
                        break;
                    }
                    thread::sleep(poll_interval);
                }
                Err(e) => return Err(format!("failed to poll scheduler state: {e}")),
            }
        }

        self.child = None;
        self.pid = None;
        Ok(())
    }

    fn restart(&mut self) -> Result<(), String> {
        self.stop()?;
        self.start()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Returns a unique temp path per test so parallel runs cannot collide.
    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cerynth-{name}-{}", std::process::id()))
    }

    fn write_heartbeat(path: &Path, timestamp: u64) {
        std::fs::write(
            path,
            format!("{{\"pid\": 123, \"profile\": \"interactive\", \"heartbeat\": {timestamp}}}"),
        )
        .unwrap();
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn heartbeat_is_fresh_when_timestamp_recent() {
        let path = temp_file("heartbeat-fresh");
        write_heartbeat(&path, now());

        assert!(heartbeat_is_fresh(&path));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn heartbeat_is_stale_when_timestamp_old() {
        let path = temp_file("heartbeat-stale");
        write_heartbeat(&path, now().saturating_sub(HEARTBEAT_TIMEOUT_SECS + 60));

        assert!(!heartbeat_is_fresh(&path));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn heartbeat_missing_or_malformed_is_not_fresh() {
        let missing = temp_file("heartbeat-missing");
        let _ = std::fs::remove_file(&missing);
        assert!(!heartbeat_is_fresh(&missing));

        let malformed = temp_file("heartbeat-malformed");
        std::fs::write(&malformed, "not json").unwrap();
        assert!(!heartbeat_is_fresh(&malformed));

        let _ = std::fs::remove_file(&malformed);
    }

    #[test]
    fn scx_state_reads_trimmed_contents() {
        let path = temp_file("scx-state");
        std::fs::write(&path, "scx_rustland\n").unwrap();

        assert_eq!(read_scx_state_from(&path), Some("scx_rustland".to_string()));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn scx_state_is_none_when_unavailable() {
        let missing = temp_file("scx-state-missing");
        let _ = std::fs::remove_file(&missing);

        assert_eq!(read_scx_state_from(&missing), None);
    }

    #[test]
    fn start_launches_scheduler_and_records_pid() {
        let scheduler = PathBuf::from("tests/fake_scheduler.sh");

        let mut backend = ScxBackend::new(scheduler, Profile::Interactive);

        assert!(backend.start().is_ok());
        assert!(backend.pid.is_some());

        let pid = backend.pid.unwrap();

        assert!(pid > 0);

        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }

    #[test]
    fn restart_stops_old_scheduler_and_starts_new_one() {
        let scheduler = PathBuf::from("tests/fake_scheduler.sh");

        let mut backend = ScxBackend::new(scheduler, Profile::Interactive);

        backend.start().expect("initial start should succeed");

        let first_pid = backend.pid.expect("first PID should exist");

        backend.restart().expect("restart should succeed");

        let second_pid = backend.pid.expect("second PID should exist");

        assert_ne!(first_pid, second_pid);

        backend.stop().expect("final stop should succeed");

        assert!(backend.pid.is_none());
        assert!(backend.child.is_none());
    }

    #[test]
    fn pause_resume_adaptation_updates_status() {
        let scheduler = PathBuf::from("tests/fake_scheduler.sh");

        let mut backend = ScxBackend::new(scheduler, Profile::Balanced);

        assert!(backend.status().unwrap().adaptation_enabled);

        backend.pause_adaptation().expect("pause should succeed");
        assert!(!backend.status().unwrap().adaptation_enabled);

        backend.resume_adaptation().expect("resume should succeed");
        assert!(backend.status().unwrap().adaptation_enabled);
    }

    #[test]
    fn set_profile_stopped_updates_profile_only() {
        let scheduler = PathBuf::from("tests/fake_scheduler.sh");

        let mut backend = ScxBackend::new(scheduler, Profile::Balanced);

        backend
            .set_profile(Profile::Performance)
            .expect("set_profile");

        assert_eq!(backend.get_profile().unwrap(), Profile::Performance);
        assert!(backend.pid.is_none());
    }

    #[test]
    fn set_profile_running_restarts_with_new_pid() {
        let scheduler = PathBuf::from("tests/fake_scheduler.sh");

        let mut backend = ScxBackend::new(scheduler, Profile::Interactive);

        backend.start().expect("start should succeed");
        let first_pid = backend.pid.expect("first PID should exist");

        backend
            .set_profile(Profile::Performance)
            .expect("set_profile while running should succeed");

        let second_pid = backend.pid.expect("PID after restart should exist");

        assert_ne!(first_pid, second_pid);
        assert_eq!(backend.get_profile().unwrap(), Profile::Performance);

        backend.stop().expect("final stop should succeed");
    }
}
