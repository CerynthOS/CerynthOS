use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
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

fn config_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("cerynth-{tag}-{}.toml", std::process::id()))
}

/// Starts the daemon with an isolated socket and state file, always driving
/// the fake scheduler. Returns the child plus the socket/state paths it uses.
fn start_daemon(tag: &str, extra: &[(&str, &str)]) -> (Child, PathBuf, PathBuf) {
    let socket = socket_path(tag);
    let state = state_path(tag);

    // Clear any stale socket from a previous run.
    let _ = std::fs::remove_file(&socket);

    // SCX lifecycle tests provide a fake sched_ext state path.
    // Those tests should explicitly use the SCX backend.
    //
    // Normal control-plane tests use the safe Mock backend.
    let use_scx = extra
        .iter()
        .any(|(key, _)| *key == "CERYNTH_SCX_STATE_PATH");

    let backend = if use_scx { "scx" } else { "mock" };

    let config = config_path(tag);

    std::fs::write(
        &config,
        format!(
            r#"
default_profile = "balanced"
adaptation_enabled = false
adaptation_mode = "off"
scheduler_backend = "{backend}"
auto_start = true
"#
        ),
    )
    .expect("failed to write test config");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cerynthd"));

    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("CERYNTH_SCX_BINARY", fake_scheduler())
        .env("CERYNTH_SOCKET", &socket)
        .env("CERYNTH_STATE", &state)
        .env("CERYNTH_CONFIG", &config);

    for (key, value) in extra {
        cmd.env(key, value);
    }

    let child = cmd.spawn().expect("failed to start daemon");

    // Give the daemon time to bind its socket and auto-start.
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

/// Writes a fake `sched_ext` state file and returns its path.
fn write_scx_state(name: &str) -> PathBuf {
    let path = temp_path(name);
    std::fs::write(&path, "enabled\n").unwrap();
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
        .env("CERYNTH_STATE", &state)
        .env("CERYNTH_CONFIG", config_path("profile-persist"));
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
        stdout.contains("sched_ext state : enabled"),
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

#[test]
fn scx_repeated_profile_transitions_keep_matching_heartbeat() {
    let scx_state = write_scx_state("profiles-state");
    let heartbeat = temp_path("profiles-heartbeat");
    let (mut daemon, socket, _) = start_daemon(
        "profile-transitions",
        &[
            ("CERYNTH_SCX_STATE_PATH", scx_state.to_str().unwrap()),
            ("CERYNTH_HEARTBEAT_PATH", heartbeat.to_str().unwrap()),
        ],
    );
    for _ in 0..2 {
        for profile in ["balanced", "interactive", "performance", "background"] {
            let result = ctl(&socket, &["profile", "set", profile]);
            assert!(!String::from_utf8_lossy(&result.stderr).contains('✗'));
            let status = ctl(&socket, &["status"]);
            let text = String::from_utf8_lossy(&status.stdout);
            assert!(text.contains("Running         : Yes"), "{text}");
            assert!(text.contains("Heartbeat       : OK"), "{text}");
            let record: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&heartbeat).unwrap()).unwrap();
            assert_eq!(record["profile"].as_str(), Some(profile));
        }
    }
    stop_daemon(&mut daemon, &socket);
    let _ = std::fs::remove_file(scx_state);
    let _ = std::fs::remove_file(heartbeat);
}

#[test]
fn scx_failed_child_is_reaped_before_starting_a_new_process() {
    let scx_state = write_scx_state("process-exit-state");
    let heartbeat = temp_path("process-exit-heartbeat");
    let (mut daemon, socket, _) = start_daemon(
        "process-exit",
        &[
            ("CERYNTH_SCX_STATE_PATH", scx_state.to_str().unwrap()),
            ("CERYNTH_HEARTBEAT_PATH", heartbeat.to_str().unwrap()),
        ],
    );
    let old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&heartbeat).unwrap()).unwrap();
    let pid = old["pid"].as_u64().unwrap();
    assert!(
        std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let status = ctl(&socket, &["status"]);
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(text.contains("Running         : No"), "{text}");
    assert!(text.contains("stale/missing"), "{text}");
    let start = ctl(&socket, &["start"]);
    assert!(!String::from_utf8_lossy(&start.stderr).contains('✗'));
    let new: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&heartbeat).unwrap()).unwrap();
    assert_ne!(new["pid"], old["pid"]);
    let status = ctl(&socket, &["status"]);
    assert!(String::from_utf8_lossy(&status.stdout).contains("Heartbeat       : OK"));
    stop_daemon(&mut daemon, &socket);
    let _ = std::fs::remove_file(scx_state);
    let _ = std::fs::remove_file(heartbeat);
}

#[test]
fn failed_profile_switch_restores_previous_healthy_profile() {
    let scx_state = write_scx_state("rollback-state");
    let heartbeat = temp_path("rollback-heartbeat");
    let (mut daemon, socket, state) = start_daemon(
        "rollback",
        &[
            ("CERYNTH_SCX_STATE_PATH", scx_state.to_str().unwrap()),
            ("CERYNTH_HEARTBEAT_PATH", heartbeat.to_str().unwrap()),
            ("CERYNTH_FAKE_FAIL_PROFILE", "performance"),
        ],
    );
    let result = ctl(&socket, &["profile", "set", "performance"]);
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("restored"), "{error}");
    let result = ctl(&socket, &["status"]);
    let status = String::from_utf8_lossy(&result.stdout);
    assert!(
        status.contains("Balanced")
            && status.contains("Running         : Yes")
            && status.contains("Heartbeat       : OK"),
        "{status}"
    );
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(state).unwrap()).unwrap();
    assert_eq!(saved["adaptation_enabled"], false);
    stop_daemon(&mut daemon, &socket);
    let _ = std::fs::remove_file(scx_state);
    let _ = std::fs::remove_file(heartbeat);
}

#[test]
fn failed_rollback_leaves_no_owned_scheduler_and_adaptation_paused() {
    let scx_state = write_scx_state("rollback-failure-state");
    let heartbeat = temp_path("rollback-failure-heartbeat");
    let marker = temp_path("rollback-failure-marker");
    let _ = std::fs::remove_file(&marker);
    let (mut daemon, socket, _) = start_daemon(
        "rollback-failure",
        &[
            ("CERYNTH_SCX_STATE_PATH", scx_state.to_str().unwrap()),
            ("CERYNTH_HEARTBEAT_PATH", heartbeat.to_str().unwrap()),
            ("CERYNTH_FAKE_FAIL_MARKER", marker.to_str().unwrap()),
        ],
    );
    std::fs::write(&marker, "fail subsequent starts").unwrap();
    let result = ctl(&socket, &["profile", "set", "performance"]);
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("rollback failed"), "{error}");
    let result = ctl(&socket, &["status"]);
    let status = String::from_utf8_lossy(&result.stdout);
    assert!(
        status.contains("Running         : No") && status.contains("Adaptation      : Disabled"),
        "{status}"
    );
    stop_daemon(&mut daemon, &socket);
    for file in [scx_state, heartbeat, marker] {
        let _ = std::fs::remove_file(file);
    }
}

#[test]
fn fragmented_frames_work_and_incomplete_mutations_are_rejected() {
    let (mut daemon, socket, _) = start_daemon("framing", &[]);
    let mut stream = UnixStream::connect(&socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    for part in [
        b"{\"version\":2,".as_slice(),
        b"\"correlation_id\":\"fragment\",",
        b"\"request\":{\"type\":\"status\"}}\n",
    ] {
        stream.write_all(part).unwrap();
        thread::sleep(Duration::from_millis(15));
    }
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    let response: cerynth_ipc::ResponseEnvelope = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response.correlation_id.as_deref(), Some("fragment"));
    for bytes in [
        b"{\"version\":2,\"request\":{\"type\":\"set-profile\",\"profile\":\"performance\"}}"
            .to_vec(),
        vec![b'x'; cerynth_ipc::MAX_MESSAGE_SIZE + 1],
    ] {
        let mut stream = UnixStream::connect(&socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream.write_all(&bytes).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        assert!(response.is_empty());
    }
    let profile = ctl(&socket, &["profile", "get"]);
    assert!(String::from_utf8_lossy(&profile.stdout).contains("Balanced"));
    stop_daemon(&mut daemon, &socket);
}

#[test]
fn sigterm_stops_owned_scheduler_and_removes_endpoint() {
    let scx_state = write_scx_state("sigterm-state");
    let heartbeat = temp_path("sigterm-heartbeat");
    let (mut daemon, socket, _) = start_daemon(
        "sigterm",
        &[
            ("CERYNTH_SCX_STATE_PATH", scx_state.to_str().unwrap()),
            ("CERYNTH_HEARTBEAT_PATH", heartbeat.to_str().unwrap()),
        ],
    );
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&heartbeat).unwrap()).unwrap();
    let scheduler_pid = record["pid"].as_u64().unwrap();
    assert!(
        Command::new("kill")
            .args(["-TERM", &daemon.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while daemon.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            let _ = daemon.kill();
            panic!("daemon ignored SIGTERM");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!socket.exists());
    assert!(!PathBuf::from(format!("/proc/{scheduler_pid}")).exists());
    let _ = std::fs::remove_file(scx_state);
    let _ = std::fs::remove_file(heartbeat);
}

#[test]
fn second_daemon_cannot_replace_live_endpoint_or_regular_file() {
    let (mut daemon, socket, _) = start_daemon("exclusive", &[]);
    let mut second = Command::new(env!("CARGO_BIN_EXE_cerynthd"))
        .env("CERYNTH_SOCKET", &socket)
        .env("CERYNTH_CONFIG", config_path("exclusive"))
        .env("CERYNTH_STATE", state_path("exclusive-second"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(200));
    let exited = second.try_wait().unwrap();
    if exited.is_none() {
        let _ = second.kill();
        let _ = second.wait();
    }
    assert!(exited.is_some_and(|exit| !exit.success()));
    assert!(ctl(&socket, &["status"]).status.success());
    stop_daemon(&mut daemon, &socket);
    let _ = std::fs::remove_file(&socket);
    std::fs::write(&socket, b"keep this file").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cerynthd"))
        .env("CERYNTH_SOCKET", &socket)
        .env("CERYNTH_CONFIG", config_path("exclusive"))
        .env("CERYNTH_STATE", state_path("exclusive-file"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(std::fs::read(&socket).unwrap(), b"keep this file");
    std::fs::remove_file(socket).unwrap();
}
