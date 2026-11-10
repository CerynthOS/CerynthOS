//! Local trusted control client. Request files use the versioned Guard protocol.
use cerynth_guardd::{
    client::Client,
    protocol::{Request, Response},
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
struct Cli {
    #[arg(long, default_value = "/run/cerynth/guard/guard.sock")]
    socket: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Status,
    Pending,
    VerifyAudit,
    Request { file: PathBuf },
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let request = match cli.command {
        Command::Status => Request::Status,
        Command::Pending => Request::PendingApprovals,
        Command::VerifyAudit => Request::VerifyAudit,
        Command::Request { file } => serde_json::from_slice(&std::fs::read(file)?)?,
    };
    let response = Client::new(cli.socket)
        .request(&request, std::time::Duration::from_secs(30))
        .await?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    if matches!(response, Response::Error { .. }) {
        std::process::exit(1);
    }
    Ok(())
}
