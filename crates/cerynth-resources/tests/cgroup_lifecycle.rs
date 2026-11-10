//! Integration tests for cgroup v2 session control.
//!
//! The privileged cases are skipped with a clear message when the host does
//! not expose a writable cgroup subtree. On a normal systemd host they run
//! unprivileged against the user manager's delegated subtree; in the dev VM
//! they run as root.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cerynth_resources::{CgroupRoot, Enforcement, ResourceLimits, SessionCgroup, SupportMatrix};
use uuid::Uuid;

fn writable_root() -> Option<CgroupRoot> {
    if let Ok(path) = std::env::var("CERYNTH_TEST_CGROUP_ROOT") {
        let root = CgroupRoot::at(path).ok()?;
        return root.is_writable().then_some(root);
    }

    let root = CgroupRoot::detect_writable().ok()?;
    root.is_writable().then_some(root)
}

fn skip(test: &str, reason: &str) {
    eprintln!("SKIP {test}: {reason}");
}

fn systemd_user_scope_available() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-system-running"])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn unique_unit(tag: &str) -> String {
    format!("cerynth-test-{}-{tag}", std::process::id())
}

struct ScopeGuard {
    unit: String,
    child: Child,
    cgroup: PathBuf,
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        let _ = Command::new("systemctl")
            .args(["--user", "stop", &format!("{}.scope", self.unit)])
            .output();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn scope_cgroup(unit: &str) -> Option<PathBuf> {
    let output = Command::new("systemctl")
        .args([
            "--user",
            "show",
            "-p",
            "ControlGroup",
            "--value",
            &format!("{unit}.scope"),
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    Some(Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/')))
}

fn launch_scope(unit: &str) -> Option<ScopeGuard> {
    let child = Command::new("systemd-run")
        .args([
            "--user",
            "--scope",
            "--unit",
            unit,
            "--collect",
            "sleep",
            "60",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(cgroup) = scope_cgroup(unit) {
            return Some(ScopeGuard {
                unit: unit.to_string(),
                child,
                cgroup,
            });
        }
        thread::sleep(Duration::from_millis(50));
    }

    let mut child = child;
    let _ = child.kill();
    let _ = child.wait();
    None
}

#[test]
fn support_matrix_detects_cgroup_v2() {
    let root = CgroupRoot::detect().expect("cgroup v2 mount");
    let matrix = SupportMatrix::probe(&root);
    assert!(matrix.cgroup_v2);
    assert_eq!(matrix.mount, "/sys/fs/cgroup");
    assert!(!matrix.controllers.is_empty());
    assert_eq!(matrix.limits.len(), 6);
}

#[test]
fn create_apply_and_remove_session() {
    let Some(root) = writable_root() else {
        skip(
            "create_apply_and_remove_session",
            "no writable cgroup v2 subtree (set CERYNTH_TEST_CGROUP_ROOT or run as root)",
        );
        return;
    };

    let id = Uuid::new_v4();
    let session = SessionCgroup::create(&root, id).expect("create session cgroup");
    let path = session.path().to_path_buf();
    assert!(path.exists(), "session cgroup should exist");

    let limits = ResourceLimits {
        memory_max_bytes: Some(32 * 1024 * 1024),
        pids_max: Some(8),
        ..ResourceLimits::default()
    };
    let applied = session.apply(&limits).expect("apply limits");

    let memory = applied.memory_max_bytes.expect("memory.max result");
    assert_ne!(
        memory.enforcement,
        Enforcement::Unsupported,
        "memory.max unavailable: {:?}",
        memory.note
    );
    assert_eq!(
        memory.enforcement,
        Enforcement::Enforced,
        "memory.max was not enforced: {:?}",
        memory.note
    );

    let pids = applied.pids_max.expect("pids.max result");
    assert_ne!(
        pids.enforcement,
        Enforcement::Unsupported,
        "pids.max unavailable: {:?}",
        pids.note
    );

    let usage = session.usage().expect("read usage");
    assert_eq!(usage.session_id, id);
    assert!(
        usage.pids.is_empty(),
        "new session should have no processes"
    );
    assert_eq!(usage.pids_usage.current, 0);

    session.remove().expect("remove session cgroup");
    assert!(!path.exists(), "session cgroup should be gone");
}

#[test]
fn systemd_scope_accounts_and_kills_the_tree() {
    if !systemd_user_scope_available() {
        skip(
            "systemd_scope_accounts_and_kills_the_tree",
            "systemctl --user is not running",
        );
        return;
    }

    let unit = unique_unit("accounting");
    let Some(scope) = launch_scope(&unit) else {
        skip(
            "systemd_scope_accounts_and_kills_the_tree",
            "could not launch a transient user scope",
        );
        return;
    };

    let id = Uuid::new_v4();
    let session = SessionCgroup::open(&scope.cgroup, id).expect("open scope cgroup");

    let limits = ResourceLimits {
        memory_max_bytes: Some(32 * 1024 * 1024),
        pids_max: Some(4),
        ..ResourceLimits::default()
    };
    let applied = session.apply(&limits).expect("apply limits");

    let memory = applied.memory_max_bytes.expect("memory.max result");
    if memory.enforcement == Enforcement::Unsupported {
        skip(
            "systemd_scope_accounts_and_kills_the_tree",
            "memory controller is not available in the user scope",
        );
        return;
    }

    let usage = session.usage().expect("read usage");
    assert!(
        !usage.pids.is_empty(),
        "scope should contain the sleep process"
    );
    assert!(usage.pids_usage.current >= 1);
    assert!(
        usage.memory.current_bytes > 0 || usage.memory.peak_bytes.unwrap_or(0) > 0,
        "scope should have accounted memory"
    );

    session.kill().expect("kill the scope tree");
    assert!(
        session
            .wait_empty(Duration::from_secs(5))
            .expect("wait for empty cgroup"),
        "scope should be empty after kill"
    );
}

#[test]
fn cpu_limit_outcome_is_classified() {
    let Some(root) = writable_root() else {
        skip(
            "cpu_limit_outcome_is_classified",
            "no writable cgroup v2 subtree",
        );
        return;
    };

    let session = SessionCgroup::create(&root, Uuid::new_v4()).expect("create session cgroup");
    let limits = ResourceLimits {
        cpu_max: Some(cerynth_resources::CpuMax::from_percent(50)),
        ..ResourceLimits::default()
    };
    let applied = session.apply(&limits).expect("apply cpu.max");
    let cpu = applied.cpu_max.expect("cpu.max result");

    // Either the controller works, or the failure is explicitly classified as
    // observed/unsupported with a reason. Silently pretending is the bug.
    match cpu.enforcement {
        Enforcement::Enforced => {
            assert_eq!(
                cpu.applied,
                Some(cerynth_resources::CpuMax::from_percent(50))
            );
        }
        Enforcement::Observed | Enforcement::Unsupported => {
            assert!(cpu.note.is_some(), "non-enforced limit needs a reason");
        }
    }

    session.remove().expect("remove session cgroup");
}

#[test]
fn invalid_limits_are_rejected_before_any_cgroup_write() {
    let Some(root) = writable_root() else {
        skip(
            "invalid_limits_are_rejected_before_any_cgroup_write",
            "no writable cgroup v2 subtree",
        );
        return;
    };

    let session = SessionCgroup::create(&root, Uuid::new_v4()).expect("create session cgroup");
    if !session.path().join("memory.max").exists() {
        skip(
            "invalid_limits_are_rejected_before_any_cgroup_write",
            "memory controller is not available in this subtree",
        );
        session.remove().expect("remove session cgroup");
        return;
    }

    // A valid limit listed before the invalid one. If validation did not run
    // first, this write would land and change the file.
    let limits = ResourceLimits {
        memory_max_bytes: Some(32 * 1024 * 1024),
        pids_max: Some(0),
        ..ResourceLimits::default()
    };

    let error = session
        .apply(&limits)
        .expect_err("pids_max = 0 must be rejected");
    let message = format!("{error:#}");
    assert!(
        message.contains("pids.max"),
        "error should name the offending limit: {message}"
    );

    let memory_max =
        std::fs::read_to_string(session.path().join("memory.max")).expect("read memory.max");
    assert_eq!(
        memory_max.trim(),
        "max",
        "validation must happen before the first cgroup write"
    );

    session.remove().expect("remove session cgroup");
}
