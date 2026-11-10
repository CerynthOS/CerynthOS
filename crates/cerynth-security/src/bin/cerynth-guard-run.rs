//! Launch a command inside a Guard sandbox built from a session's
//! capabilities. This is the development launcher; guardd uses the same
//! profile code and additionally binds the broker and egress sockets.
//!
//!   cerynth-guard-run --workspace DIR [--caps caps.json] -- CMD [ARGS...]
//!   cerynth-guard-run --session session.json -- CMD [ARGS...]
//!   cerynth-guard-run --workspace DIR --print -- CMD   # show the command line

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use cerynth_security::{CapabilitySet, GuardSession, HostPaths, SandboxProfile, SessionMode};
use clap::Parser;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(name = "cerynth-guard-run", about = "Run a command in a Cerynth Guard sandbox")]
struct Cli {
    /// Session description (JSON `GuardSession`).
    #[arg(long, conflicts_with_all = ["workspace", "caps"])]
    session: Option<PathBuf>,
    /// Workspace directory (mounted read-write at /workspace).
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Capabilities (JSON list). Default: none, i.e. workspace only.
    #[arg(long)]
    caps: Option<PathBuf>,
    /// Guard broker socket on the host, bound to /run/agent/guard.sock.
    #[arg(long)]
    guard_socket: Option<PathBuf>,
    /// Egress proxy socket on the host, bound to /run/agent/egress.sock.
    #[arg(long)]
    egress_socket: Option<PathBuf>,
    /// Print the command line instead of running it.
    #[arg(long)]
    print: bool,
    #[arg(last = true, required = true)]
    cmd: Vec<String>,
}

fn load(cli: &Cli) -> anyhow::Result<GuardSession> {
    if let Some(p) = &cli.session {
        return Ok(serde_json::from_slice(&std::fs::read(p)?)?);
    }
    let workspace = cli.workspace.clone().ok_or_else(|| anyhow::anyhow!("--workspace or --session is required"))?;
    let capabilities: CapabilitySet = match &cli.caps {
        Some(p) => serde_json::from_slice(&std::fs::read(p)?)?,
        None => CapabilitySet::default(),
    };
    Ok(GuardSession { session_id: Uuid::new_v4(), parent_session_id: None, mode: SessionMode::Managed, workspace, capabilities, harness: None })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let run = || -> anyhow::Result<Vec<String>> {
        let session = load(&cli)?;
        let profile = SandboxProfile::for_session(&session).map_err(|e| anyhow::anyhow!(e))?;
        let guard_exec = std::env::current_exe()?
            .parent()
            .map(|d| d.join("cerynth-guard-exec"))
            .filter(|p| p.exists())
            .ok_or_else(|| anyhow::anyhow!("cerynth-guard-exec not found next to cerynth-guard-run"))?;
        let host = HostPaths { guard_exec, guard_socket: cli.guard_socket.clone(), egress_socket: cli.egress_socket.clone() };
        Ok(profile.command_line(&host, &cli.cmd))
    };
    let argv = match run() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("cerynth-guard-run: {e:#}");
            return ExitCode::from(2);
        }
    };
    if cli.print {
        println!("{}", argv.join(" "));
        return ExitCode::SUCCESS;
    }
    let err = Command::new(&argv[0]).args(&argv[1..]).exec();
    eprintln!("cerynth-guard-run: exec bwrap: {err}");
    ExitCode::from(127)
}
