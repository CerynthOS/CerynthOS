use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cerynth_ipc::{Profile, SchedulerBackend, SchedulerStatus};

use crate::backend::Backend;

/// How old a heartbeat timestamp may be before the scheduler is considered
/// unhealthy.
const HEARTBEAT_TIMEOUT_SECS: u64 = 10;

/// Maximum amount of time start() waits for the scheduler to become healthy.
const START_TIMEOUT_SECS: u64 = 15;

/// Default sched_ext subsystem state file.
const DEFAULT_SCX_STATE_PATH: &str = "/sys/kernel/sched_ext/state";

/// Default scheduler heartbeat file.
const DEFAULT_HEARTBEAT_PATH: &str = "/run/cerynth/scheduler.json";

fn scx_state_path() -> PathBuf {
    std::env::var_os("CERYNTH_SCX_STATE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SCX_STATE_PATH))
}

fn heartbeat_path() -> PathBuf {
    std::env::var_os("CERYNTH_HEARTBEAT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_HEARTBEAT_PATH))
}

fn read_scx_state_from(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let state = contents.trim();

    if state.is_empty() || state == "none" {
        None
    } else {
        Some(state.to_string())
    }
}

fn sched_ext_is_enabled(path: &Path) -> bool {
    read_scx_state_from(path).as_deref() == Some("enabled")
}

fn heartbeat_is_fresh(path: &Path, expected_pid: u32) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };

    let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return false;
    };

    if json.get("pid").and_then(serde_json::Value::as_u64) != Some(u64::from(expected_pid))
        || json.get("running").and_then(serde_json::Value::as_bool) != Some(true)
        || json.get("healthy").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return false;
    }

    let Some(timestamp) = json
        .get("heartbeat")
        .and_then(serde_json::Value::as_u64)
    else {
        return false;
    };

    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return false;
    };

    timestamp <= now.as_secs()
        && now.as_secs() - timestamp <= HEARTBEAT_TIMEOUT_SECS
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

    pub fn with_adaptation(mut self, adaptation_enabled: bool) -> Self {
        self.adaptation_enabled = adaptation_enabled;
        self
    }

    /// Reconcile the tracked child with reality.
    ///
    /// If the scheduler exited unexpectedly, clear the stored PID and Child
    /// so that the backend can be started again.
    fn reap_child(&mut self) -> Result<(), String> {
        let Some(child) = self.child.as_mut() else {
            self.pid = None;
            return Ok(());
        };

        match child.try_wait() {
            Ok(Some(_status)) => {
                self.child = None;
                self.pid = None;
            }

            Ok(None) => {
                // Child is still alive.
            }

            Err(error) => {
                return Err(format!(
                    "failed to inspect scheduler process: {error}"
                ));
            }
        }

        Ok(())
    }

    /// Returns whether the scheduler is currently healthy enough to be used.
    fn scheduler_is_healthy(&self) -> bool {
        let Some(pid) = self.pid else {
            return false;
        };

        let process_alive = unsafe {
            libc::kill(pid as i32, 0) == 0
        };

        if !process_alive {
            return false;
        }

        let sched_ext_enabled = sched_ext_is_enabled(&scx_state_path());

        let heartbeat_ok = heartbeat_is_fresh(&heartbeat_path(), pid);

        process_alive && sched_ext_enabled && heartbeat_ok
    }

    fn ensure_healthy_startup(&mut self, timeout: Duration) -> Result<(), String> {
        if let Err(error) = self.wait_until_healthy(timeout) {
            let _ = self.stop();
            return Err(error);
        }
        Ok(())
    }

    /// Wait until the spawned scheduler is alive and healthy.
    fn wait_until_healthy(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            self.reap_child()?;

            if self.pid.is_none() {
                return Err(
                    "scheduler exited before becoming healthy".to_string()
                );
            }

            if self.scheduler_is_healthy() {
                return Ok(());
            }

            thread::sleep(Duration::from_millis(100));
        }

        Err(format!("scheduler did not become healthy within {} seconds", timeout.as_secs_f64()))
    }
}

impl Backend for ScxBackend {
    fn status(&mut self) -> Result<SchedulerStatus, String> {
        self.reap_child()?;

        let running = self.pid.is_some();

        let sched_ext_state =
            read_scx_state_from(&scx_state_path());

        let heartbeat_ok =
            self.pid.is_some_and(|pid| heartbeat_is_fresh(&heartbeat_path(), pid));

        Ok(SchedulerStatus {
            profile: self.profile.clone(),
            adaptation_enabled: self.adaptation_enabled,
            backend: SchedulerBackend::Scx,
            running,
            sched_ext_state,
            heartbeat_ok,
        })
    }

    fn get_profile(&self) -> Result<Profile, String> {
        Ok(self.profile.clone())
    }

    fn set_profile(&mut self, profile: Profile) -> Result<(), String> {
        self.reap_child()?;

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
        self.reap_child()?;

        if self.pid.is_some() {
            return Err(
                "scheduler is already running".to_string()
            );
        }

        if !self.scheduler_binary.exists() {
            return Err(format!(
                "scheduler binary not found: {}",
                self.scheduler_binary.display()
            ));
        }

        let child = Command::new(&self.scheduler_binary)
            .arg("--profile")
            .arg(self.profile.to_string())
            .spawn()
            .map_err(|e| {
                format!("failed to spawn scheduler: {e}")
            })?;

        self.pid = Some(child.id());
        self.child = Some(child);

        // Safety gate:
        //
        // spawn() only means that the process was created.
        //
        // We require:
        //
        // 1. process is alive
        // 2. sched_ext reports an active scheduler
        // 3. heartbeat is fresh
        //
        // before reporting successful start.
        self.ensure_healthy_startup(Duration::from_secs(START_TIMEOUT_SECS))
    }

    fn stop(&mut self) -> Result<(), String> {
        self.reap_child()?;

        let Some(child) = self.child.as_mut() else {
            return Err(
                "no scheduler is currently running".to_string()
            );
        };

        let pid = self
            .pid
            .ok_or_else(|| {
                "no scheduler is currently running".to_string()
            })?;

        // First attempt graceful termination.
        if unsafe {
            libc::kill(pid as i32, libc::SIGTERM)
        } != 0
        {
            let error = std::io::Error::last_os_error();

            // If the process disappeared between reap_child() and kill(),
            // treat it as already stopped.
            if error.raw_os_error() == Some(libc::ESRCH) {
                self.child = None;
                self.pid = None;
                return Ok(());
            }

            return Err(format!(
                "failed to send SIGTERM to scheduler (pid {pid}): {error}"
            ));
        }

        let deadline =
            Instant::now() + Duration::from_secs(5);

        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,

                Ok(None) => {
                    if Instant::now() >= deadline {
                        if unsafe {
                            libc::kill(pid as i32, libc::SIGKILL)
                        } != 0
                        {
                            let error =
                                std::io::Error::last_os_error();

                            if error.raw_os_error() != Some(libc::ESRCH) {
                                return Err(format!(
                                    "failed to send SIGKILL to scheduler \
                                     (pid {pid}): {error}"
                                ));
                            }
                        }

                        child.wait().map_err(|e| {
                            format!(
                                "failed to reap scheduler after SIGKILL: {e}"
                            )
                        })?;

                        break;
                    }

                    thread::sleep(Duration::from_millis(100));
                }

                Err(error) => {
                    return Err(format!(
                        "failed to poll scheduler state: {error}"
                    ));
                }
            }
        }

        self.child = None;
        self.pid = None;

        Ok(())
    }

    fn restart(&mut self) -> Result<(), String> {
        self.reap_child()?;

        if self.pid.is_some() {
            self.stop()?;
        }

        self.start()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_is_fresh_when_timestamp_recent() {
        let path = std::env::temp_dir().join(format!(
            "cerynth-heartbeat-fresh-{}",
            std::process::id()
        ));

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        std::fs::write(
            &path,
            format!(
                "{{\"pid\":123,\"running\":true,\"healthy\":true,\"profile\":\"interactive\",\
                 \"heartbeat\":{now}}}"
            ),
        )
        .unwrap();

        assert!(heartbeat_is_fresh(&path, 123));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn heartbeat_is_stale_when_timestamp_old() {
        let path = std::env::temp_dir().join(format!(
            "cerynth-heartbeat-stale-{}",
            std::process::id()
        ));

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let old =
            now.saturating_sub(HEARTBEAT_TIMEOUT_SECS + 60);

        std::fs::write(
            &path,
            format!(
                "{{\"pid\":123,\"running\":true,\"healthy\":true,\"profile\":\"interactive\",\
                 \"heartbeat\":{old}}}"
            ),
        )
        .unwrap();

        assert!(!heartbeat_is_fresh(&path, 123));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn heartbeat_missing_or_malformed_is_not_fresh() {
        let missing = std::env::temp_dir().join(format!(
            "cerynth-heartbeat-missing-{}",
            std::process::id()
        ));

        let _ = std::fs::remove_file(&missing);

        assert!(!heartbeat_is_fresh(&missing, 123));

        let malformed = std::env::temp_dir().join(format!(
            "cerynth-heartbeat-malformed-{}",
            std::process::id()
        ));

        std::fs::write(&malformed, "not json").unwrap();

        assert!(!heartbeat_is_fresh(&malformed, 123));

        let _ = std::fs::remove_file(malformed);
    }

    #[test]
    fn heartbeat_rejects_other_process_stopped_and_future_records() {
        let path = std::env::temp_dir().join(format!("cerynth-heartbeat-contract-{}", std::process::id()));
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        for record in [
            serde_json::json!({"pid":124,"running":true,"healthy":true,"heartbeat":now}),
            serde_json::json!({"pid":123,"running":false,"healthy":true,"heartbeat":now}),
            serde_json::json!({"pid":123,"running":true,"healthy":false,"heartbeat":now}),
            serde_json::json!({"pid":123,"running":true,"healthy":true,"heartbeat":now+60}),
        ] {
            std::fs::write(&path, record.to_string()).unwrap();
            assert!(!heartbeat_is_fresh(&path, 123));
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn disabled_or_transitioning_sched_ext_is_not_healthy() {
        let path = std::env::temp_dir().join(format!("cerynth-scx-health-{}", std::process::id()));
        for state in ["disabled", "enabling", "disabling", "none", ""] {
            std::fs::write(&path, state).unwrap();
            assert!(!sched_ext_is_enabled(&path));
        }
        std::fs::write(&path, "enabled\n").unwrap();
        assert!(sched_ext_is_enabled(&path));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn startup_timeout_stops_and_reaps_unhealthy_child() {
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let mut backend = ScxBackend::new(PathBuf::from("unused"), Profile::Balanced);
        backend.pid = Some(pid);
        backend.child = Some(child);
        let started = Instant::now();
        let result = backend.ensure_healthy_startup(Duration::from_millis(100));
        assert!(result.unwrap_err().contains("did not become healthy"));
        assert!(backend.pid.is_none());
        assert!(backend.child.is_none());
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn exited_child_fails_before_startup_deadline() {
        let mut child = Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        let mut backend = ScxBackend::new(PathBuf::from("unused"), Profile::Balanced);
        backend.pid = Some(child.id());
        backend.child = Some(child);
        let result = backend.wait_until_healthy(Duration::from_secs(15));
        assert!(result.unwrap_err().contains("exited before becoming healthy"));
        assert!(backend.pid.is_none());
    }

    #[test]
    fn scx_state_reads_trimmed_contents() {
        let path = std::env::temp_dir().join(format!(
            "cerynth-scx-state-{}",
            std::process::id()
        ));

        std::fs::write(&path, "scx_rustland\n").unwrap();

        assert_eq!(
            read_scx_state_from(&path),
            Some("scx_rustland".to_string())
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn scx_state_is_none_when_unavailable() {
        let path = std::env::temp_dir().join(format!(
            "cerynth-scx-state-missing-{}",
            std::process::id()
        ));

        let _ = std::fs::remove_file(&path);

        assert_eq!(read_scx_state_from(&path), None);
    }
}
