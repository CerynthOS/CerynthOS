use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::Duration,
};

/// Absolute path to the fake scheduler, used by every integration-test daemon
/// so the full SCX lifecycle can be exercised without a real
/// `/usr/bin/cerynth-scx` on PATH.
fn fake_scheduler() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_scheduler.sh")
}

/// A unique, isolated socket path for a given test tag. Tests share a process
/// id but have distinct tags, so sockets never collide in parallel runs.
fn socket_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("cerynth-{tag}-{}.sock", std::process::id()))
}

/// A unique, isolated runtime-state path for a given test tag, so profile
/// changes in one test never overwrite another test's daemon state.
fn state_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("cerynth-{tag}-{}.json", std::process::id()))
}

/// Starts the daemon with an isolated socket and state file, always driving
/// the fake scheduler. Returns the child plus the socket/state paths it uses.
fn start_daemon(tag: &str, extra: &[(&str, &str)]) -> (Child, PathBuf, PathBuf) {
    let socket = socket_path(tag);
    let state = state_path(tag);

    // Clear any stale socket from a previous run so the bind succeeds.
    let _ = std::fs::remove_file(&socket);

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cerynthd"));
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("CERYNTH_SCX_BINARY", fake_scheduler())
        .env("CERYNTH_SOCKET", &socket)
        .env("CERYNTH_STATE", &state);
    for (key, value) in extra {
        cmd.env(key, value);
    }
    let child = cmd.spawn().expect("failed to start daemon");

    // Give the daemon a moment to bind its socket and auto-start.
    thread::sleep(Duration::from_secs(1));

    (child, socket, state)
}

/// Runs `cerynthctl` against a specific daemon's socket.
fn ctl(socket: &Path, args: &[&str]) -> Output {
    Command::new("cargo")
        .args(["run", "-p", "cerynthctl", "--"])
        .args(args)
        .env("CERYNTH_SOCKET", socket)
        .output()
        .expect("failed to run cerynthctl")
}

/// Gracefully stops the auto-started scheduler (so its fake process exits),
/// then terminates the daemon.
fn stop_daemon(daemon: &mut Child, socket: &Path) {
    let _ = ctl(socket, &["stop"]);
    let _ = daemon.kill();
    let _ = daemon.wait();
}

/// Returns a unique temp path per test for e2e health files.
fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("cerynth-e2e-{name}-{}", std::process::id()))
}

/// Writes a fake sched_ext state file and returns its path.
fn write_scx_state(name: &str) -> PathBuf {
    let path = temp_path(name);
    std::fs::write(&path, "scx_e2e_sched\n").unwrap();
    path
}

#[test]
fn daemon_starts() {
    let (mut daemon, socket, _state) = start_daemon("daemon-starts", &[]);

    assert!(daemon.try_wait().unwrap().is_none());

    stop_daemon(&mut daemon, &socket);
}

#[test]
fn cli_status_command() {
    let (mut daemon, socket, _state) = start_daemon("cli-status", &[]);

    let output = ctl(&socket, &["status"]);

    println!("Exit status: {:?}", output.status);
    println!("STDOUT:\n{}", String::from_utf8_lossy(&output.stdout));
    println!("STDERR:\n{}", String::from_utf8_lossy(&output.stderr));

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Current Profile"));
    assert!(stdout.contains("Backend"));

    stop_daemon(&mut daemon, &socket);
}

#[test]
fn cli_set_and_get_profile() {
    let (mut daemon, socket, _state) = start_daemon("cli-set-get", &[]);

    // Set profile
    let output = ctl(&socket, &["profile", "set", "interactive"]);

    assert!(output.status.success());

    // Read it back
    let output = ctl(&socket, &["profile", "get"]);

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Interactive"));

    stop_daemon(&mut daemon, &socket);
}

#[test]
fn profile_persists_after_restart() {
    // Start daemon
    let (mut daemon, socket, state) = start_daemon("profile-persist", &[]);

    // Set profile
    let output = ctl(&socket, &["profile", "set", "performance"]);

    assert!(output.status.success());

    // Stop the daemon cleanly (also stops the auto-started fake scheduler).
    let _ = ctl(&socket, &["stop"]);
    let _ = daemon.kill();
    let _ = daemon.wait();

    // Give the OS a moment to release the socket.
    thread::sleep(Duration::from_millis(500));

    // Restart the daemon on the same isolated socket and state paths.
    let _ = std::fs::remove_file(&socket);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cerynthd"));
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("CERYNTH_SCX_BINARY", fake_scheduler())
        .env("CERYNTH_SOCKET", &socket)
        .env("CERYNTH_STATE", &state);
    let mut daemon = cmd.spawn().expect("failed to restart daemon");

    thread::sleep(Duration::from_secs(1));

    // Read profile
    let output = ctl(&socket, &["profile", "get"]);

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Performance"));

    stop_daemon(&mut daemon, &socket);
}

/// Full lifecycle through the real CLI/IPC/daemon/backend against the fake
/// scheduler: auto-start, health, profile change, restart, stop, start-again,
/// and a stop-with-nothing-running error.
#[test]
fn scx_lifecycle_flow() {
    let scx_state = write_scx_state("scx-state");
    let heartbeat = temp_path("heartbeat");
    let _ = std::fs::remove_file(&heartbeat);

    let (mut daemon, socket, _state) = start_daemon(
        "scx-lifecycle",
        &[
            ("CERYNTH_SCX_STATE_PATH", scx_state.to_str().unwrap()),
            ("CERYNTH_HEARTBEAT_PATH", heartbeat.to_str().unwrap()),
        ],
    );

    // Give auto-start and the fake's heartbeat writer a moment.
    thread::sleep(Duration::from_secs(2));

    // Auto-start should have launched the fake scheduler and it should be healthy.
    let status = ctl(&socket, &["status"]);
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(stdout.contains("Running         : Yes"), "was:\n{stdout}");
    assert!(
        stdout.contains("sched_ext state : scx_e2e_sched"),
        "was:\n{stdout}"
    );
    assert!(stdout.contains("Heartbeat       : OK"), "was:\n{stdout}");

    // Changing profile while running restarts the scheduler.
    let set = ctl(&socket, &["profile", "set", "interactive"]);
    assert!(set.status.success());
    let status = ctl(&socket, &["status"]);
    assert!(
        String::from_utf8_lossy(&status.stdout).contains("Interactive"),
        "was:\n{}",
        String::from_utf8_lossy(&status.stdout)
    );

    // Restart, then stop.
    assert!(ctl(&socket, &["restart"]).status.success());
    assert!(ctl(&socket, &["stop"]).status.success());
    let status = ctl(&socket, &["status"]);
    assert!(
        String::from_utf8_lossy(&status.stdout).contains("Running         : No"),
        "was:\n{}",
        String::from_utf8_lossy(&status.stdout)
    );

    // Start again from a fully stopped state.
    assert!(ctl(&socket, &["start"]).status.success());
    let status = ctl(&socket, &["status"]);
    assert!(
        String::from_utf8_lossy(&status.stdout).contains("Running         : Yes"),
        "was:\n{}",
        String::from_utf8_lossy(&status.stdout)
    );

    // Stop, then stop again: nothing is running, so it reports an error.
    assert!(ctl(&socket, &["stop"]).status.success());
    let stop_again = ctl(&socket, &["stop"]);
    let stop_err = String::from_utf8_lossy(&stop_again.stderr);
    assert!(
        stop_err.contains("no scheduler is currently running"),
        "stderr was:\n{stop_err}"
    );

    stop_daemon(&mut daemon, &socket);
    let _ = std::fs::remove_file(&scx_state);
    let _ = std::fs::remove_file(&heartbeat);
}

/// Auto-start means a second explicit start must be rejected.
#[test]
fn scx_double_start_errors() {
    let scx_state = write_scx_state("scx-state-double");
    let heartbeat = temp_path("heartbeat-double");
    let _ = std::fs::remove_file(&heartbeat);

    let (mut daemon, socket, _state) = start_daemon(
        "scx-double-start",
        &[
            ("CERYNTH_SCX_STATE_PATH", scx_state.to_str().unwrap()),
            ("CERYNTH_HEARTBEAT_PATH", heartbeat.to_str().unwrap()),
        ],
    );

    thread::sleep(Duration::from_secs(2));

    // Auto-start already spawned the fake scheduler, so a second start errors.
    let start = ctl(&socket, &["start"]);
    let stderr = String::from_utf8_lossy(&start.stderr);
    assert!(stderr.contains("already running"), "stderr was:\n{stderr}");

    stop_daemon(&mut daemon, &socket);
    let _ = std::fs::remove_file(&scx_state);
    let _ = std::fs::remove_file(&heartbeat);
}
