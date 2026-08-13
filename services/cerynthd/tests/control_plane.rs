use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

/// A daemon under test, with its own socket and state file.
///
/// The production paths (`/run/cerynth`, `/var/lib/cerynth`) need root, and a
/// single shared socket would make these tests race each other, so every test
/// gets a private pair via the `CERYNTH_SOCKET` / `CERYNTH_STATE` overrides.
struct Fixture {
    socket: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);

        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("failed to create fixture dir");

        Self {
            socket: dir.join("cerynthd.sock"),
            state: dir.join("state.json"),
        }
    }

    fn start_daemon(&self) -> Child {
        let daemon = Command::new("cargo")
            .args(["run", "-p", "cerynthd"])
            .env("CERYNTH_SOCKET", &self.socket)
            .env("CERYNTH_STATE", &self.state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to start daemon");

        self.wait_for_socket();

        daemon
    }

    /// Polls for the socket instead of sleeping a fixed interval, so a slow
    /// debug build cannot make these tests flaky.
    fn wait_for_socket(&self) {
        for _ in 0..100 {
            if self.socket.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }

        panic!("daemon did not create socket at {}", self.socket.display());
    }

    fn ctl(&self, args: &[&str]) -> std::process::Output {
        Command::new("cargo")
            .args(["run", "-p", "cerynthctl", "--"])
            .args(args)
            .env("CERYNTH_SOCKET", &self.socket)
            .output()
            .expect("failed to run cerynthctl")
    }
}

#[test]
fn daemon_starts() {
    let fixture = Fixture::new("daemon_starts");
    let mut daemon = fixture.start_daemon();

    assert!(daemon.try_wait().unwrap().is_none());

    let _ = daemon.kill();
    let _ = daemon.wait();
}

#[test]
fn cli_status_command() {
    let fixture = Fixture::new("cli_status_command");
    let mut daemon = fixture.start_daemon();

    let output = fixture.ctl(&["status"]);

    assert!(
        output.status.success(),
        "cerynthctl status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Current Profile"), "got: {stdout}");
    assert!(stdout.contains("Backend"), "got: {stdout}");

    let _ = daemon.kill();
    let _ = daemon.wait();
}

#[test]
fn cli_set_and_get_profile() {
    let fixture = Fixture::new("cli_set_and_get_profile");
    let mut daemon = fixture.start_daemon();

    let output = fixture.ctl(&["profile", "set", "interactive"]);

    assert!(
        output.status.success(),
        "profile set failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = fixture.ctl(&["profile", "get"]);

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Interactive"), "got: {stdout}");

    let _ = daemon.kill();
    let _ = daemon.wait();
}

#[test]
fn profile_persists_after_restart() {
    let fixture = Fixture::new("profile_persists_after_restart");
    let mut daemon = fixture.start_daemon();

    let output = fixture.ctl(&["profile", "set", "performance"]);

    assert!(
        output.status.success(),
        "profile set failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = daemon.kill();
    let _ = daemon.wait();

    // The daemon is killed, so it cannot unlink its own socket.
    let _ = std::fs::remove_file(&fixture.socket);

    let mut daemon = fixture.start_daemon();

    let output = fixture.ctl(&["profile", "get"]);

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Performance"), "got: {stdout}");

    let _ = daemon.kill();
    let _ = daemon.wait();
}
