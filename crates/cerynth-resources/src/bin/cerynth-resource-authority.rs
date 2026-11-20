use anyhow::Result;
use cerynth_resources::authority::{Authority, Config};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Owner-only shared reservation domain; no automatic workload adoption")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Init {
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        policy: PathBuf,
    },
    Status {
        #[arg(long)]
        directory: PathBuf,
    },
}
fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Init { directory, policy } => {
            let config: Config = serde_json::from_slice(&std::fs::read(policy)?)?;
            Authority::initialize(&directory, config)?;
        }
        Command::Status { directory } => println!(
            "{}",
            serde_json::to_string_pretty(&Authority::open(&directory)?.view()?)?
        ),
    }
    Ok(())
}
