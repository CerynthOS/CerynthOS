use std::{
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::Duration,
};

// All tests in this file drive a real cerynthd daemon bound to a single,
// fixed, non-configurable Unix socket path. Running them in parallel (the
// default) lets multiple daemons/clients race on that one socket, so this
// lock forces the tests in this file to run one at a time.
static DAEMON_LOCK: Mutex<()> = Mutex::new(());

fn start_daemon() -> Child {
    let daemon = Command::new("cargo")
        .args(["run", "-p", "cerynthd"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start daemon");

    thread::sleep(Duration::from_secs(1));

    daemon
}

#[test]
fn daemon_starts() {
    let _guard = DAEMON_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut daemon = start_daemon();

    assert!(daemon.try_wait().unwrap().is_none());

    let _ = daemon.kill();
    let _ = daemon.wait();
}
#[test]
fn cli_status_command() {
    let _guard = DAEMON_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut daemon = start_daemon();

    let output = Command::new("cargo")
        .args(["run", "-p", "cerynthctl", "--", "status"])
        .output()
        .expect("failed to run cerynthctl");

    println!("Exit status: {:?}", output.status);
    println!("STDOUT:\n{}", String::from_utf8_lossy(&output.stdout));
    println!("STDERR:\n{}", String::from_utf8_lossy(&output.stderr));

    println!("SET STDOUT:\n{}", String::from_utf8_lossy(&output.stdout));
    println!("SET STDERR:\n{}", String::from_utf8_lossy(&output.stderr));
    println!("SET STATUS: {:?}", output.status);

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Current Profile"));
    assert!(stdout.contains("Backend"));

    let _ = daemon.kill();
    let _ = daemon.wait();
}

#[test]
fn cli_set_and_get_profile() {
    let _guard = DAEMON_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut daemon = start_daemon();

    // Set profile
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "cerynthctl",
            "--",
            "profile",
            "set",
            "interactive",
        ])
        .output()
        .expect("failed to set profile");

    assert!(output.status.success());

    // Read it back
    let output = Command::new("cargo")
        .args(["run", "-p", "cerynthctl", "--", "profile", "get"])
        .output()
        .expect("failed to get profile");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Interactive"));

    let _ = daemon.kill();
    let _ = daemon.wait();
}

#[test]
fn profile_persists_after_restart() {
    let _guard = DAEMON_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Start daemon
    let mut daemon = start_daemon();

    // Set profile
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "cerynthctl",
            "--",
            "profile",
            "set",
            "performance",
        ])
        .output()
        .expect("failed to set profile");

    assert!(output.status.success());

    // Stop daemon
    let _ = daemon.kill();
    let _ = daemon.wait();

    // Give the OS a moment to release the socket
    thread::sleep(Duration::from_millis(500));

    // Restart daemon
    let mut daemon = start_daemon();

    // Read profile
    let output = Command::new("cargo")
        .args(["run", "-p", "cerynthctl", "--", "profile", "get"])
        .output()
        .expect("failed to get profile");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Performance"));

    let _ = daemon.kill();
    let _ = daemon.wait();
}
