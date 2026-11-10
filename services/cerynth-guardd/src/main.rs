//! `cerynth-guardd` offline commands.
//!
//!   cerynth-guardd check-config [/etc/cerynth/guard/guardd.toml]
//!   cerynth-guardd verify-audit [/var/lib/cerynth/guard/audit.jsonl]
//!
//! `serve` starts the experimental local Unix socket service.

use std::path::PathBuf;
use std::process::ExitCode;

use cerynth_guardd::config::GuardConfig;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "cerynth-guardd", about = "Cerynth Guard daemon")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Load and validate the configuration, policy and vault.
    CheckConfig {
        #[arg(default_value = "/etc/cerynth/guard/guardd.toml")]
        path: PathBuf,
    },
    /// Verify the audit log's hash chain.
    VerifyAudit {
        #[arg(default_value = "/var/lib/cerynth/guard/audit.jsonl")]
        path: PathBuf,
    },
    /// Run the local Guard service.
    Serve {
        #[arg(default_value = "/etc/cerynth/guard/guardd.toml")]
        path: PathBuf,
    },
}

fn main() -> ExitCode {
    match Cli::parse().cmd {
        Cmd::CheckConfig { path } => {
            let result = GuardConfig::load(&path).and_then(|c| c.build().map(|g| (c, g)));
            match result {
                Ok((cfg, guard)) => {
                    println!("ok: {}", path.display());
                    println!("  policy:       {}", guard.cfg.policy_source);
                    println!("  credentials:  {}", guard.vault.names().count());
                    println!("  audit log:    {}", guard.audit_path().display());
                    println!("  approval ttl: {}s", cfg.approval_ttl_secs);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("invalid: {e:#}");
                    ExitCode::from(1)
                }
            }
        }
        Cmd::VerifyAudit { path } => match cerynth_security::audit::verify(&path) {
            Ok(v) => {
                println!("ok: {} records, last hash {}", v.records, v.last_hash);
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("BROKEN: {e}");
                ExitCode::from(1)
            }
        },
        Cmd::Serve { path } => {
            let result = GuardConfig::load(&path)
                .and_then(|cfg| cfg.build())
                .and_then(|guard| {
                    tokio::runtime::Runtime::new()?.block_on(cerynth_guardd::server::serve(guard))
                });
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("guardd: {e:#}");
                    ExitCode::from(1)
                }
            }
        }
    }
}
