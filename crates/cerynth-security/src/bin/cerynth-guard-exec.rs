//! Runs as the first process inside a Guard sandbox (bwrap execs it).
//!
//! 1. If the session has egress, start a loopback forwarder:
//!    127.0.0.1:<port> inside the sandbox → the Guard egress socket.
//! 2. Apply Landlock (L4): only the listed paths are reachable.
//! 3. Apply seccomp (L5).
//! 4. exec the agent command. Everything it spawns inherits 2 and 3.
//!
//! Fails closed: if Landlock is unsupported or only partially enforced, it
//! refuses to run the agent unless CERYNTH_GUARD_ALLOW_PARTIAL_LANDLOCK=1.

use std::io;
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use cerynth_security::confine::{LandlockLevel, apply_landlock, apply_seccomp};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "cerynth-guard-exec", about = "Confine this process with Landlock + seccomp, then exec CMD")]
struct Cli {
    /// Read-only path (repeatable).
    #[arg(long = "ro")]
    ro: Vec<PathBuf>,
    /// Read-write path (repeatable).
    #[arg(long = "rw")]
    rw: Vec<PathBuf>,
    /// Unix socket of the Guard egress proxy, inside the sandbox.
    #[arg(long, requires = "egress_port")]
    egress_socket: Option<PathBuf>,
    #[arg(long)]
    egress_port: Option<u16>,
    /// Internal: run only the loopback forwarder.
    #[arg(long, hide = true)]
    forward_only: bool,
    #[arg(last = true, required_unless_present = "forward_only")]
    cmd: Vec<String>,
}

fn pipe(mut from: impl io::Read, mut to: impl io::Write) {
    let _ = io::copy(&mut from, &mut to);
}

fn handle(tcp: TcpStream, socket: &PathBuf) -> io::Result<()> {
    let unix = UnixStream::connect(socket)?;
    let (tcp_r, unix_w) = (tcp.try_clone()?, unix.try_clone()?);
    std::thread::spawn(move || {
        pipe(tcp_r, unix_w);
    });
    pipe(unix, tcp);
    Ok(())
}

fn forward(port: u16, socket: &PathBuf) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    for conn in listener.incoming().flatten() {
        let socket = socket.clone();
        std::thread::spawn(move || {
            let _ = handle(conn, &socket);
        });
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let fail = |msg: String| {
        eprintln!("cerynth-guard-exec: {msg}");
        ExitCode::from(126)
    };

    // Confine first, so the forwarder child is confined too.
    match apply_landlock(&cli.ro, &cli.rw) {
        Ok(LandlockLevel::Full) => {}
        Ok(level) if std::env::var_os("CERYNTH_GUARD_ALLOW_PARTIAL_LANDLOCK").is_some() => {
            eprintln!("cerynth-guard-exec: warning: Landlock {level:?}, continuing because CERYNTH_GUARD_ALLOW_PARTIAL_LANDLOCK is set");
        }
        Ok(level) => return fail(format!("Landlock is {level:?} on this kernel; refusing to run the agent with fewer layers")),
        Err(e) => return fail(format!("Landlock failed: {e:#}")),
    }
    if let Err(e) = apply_seccomp() {
        return fail(format!("seccomp failed: {e:#}"));
    }

    if let (Some(socket), Some(port)) = (&cli.egress_socket, cli.egress_port) {
        if cli.forward_only {
            return match forward(port, socket) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => fail(format!("forwarder: {e}")),
            };
        }
        // A separate process, so exec'ing the agent below doesn't kill it.
        let me = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/run/agent/cerynth-guard-exec"));
        if let Err(e) = Command::new(me)
            .args(["--forward-only", "--egress-port", &port.to_string(), "--egress-socket"])
            .arg(socket)
            .spawn()
        {
            return fail(format!("could not start the egress forwarder: {e}"));
        }
        // Wait until the forwarder listens, so the agent's first request works.
        for _ in 0..100 {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    let Some((prog, args)) = cli.cmd.split_first() else {
        return fail("no command".into());
    };
    let err = Command::new(prog).args(args).exec();
    fail(format!("exec {prog}: {err}"))
}
