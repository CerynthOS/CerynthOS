use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    about = "Shared inference coordinator; configure grants through the private control socket"
)]
struct Cli {
    #[arg(long, env = "CERYNTH_INFERENCE_DIR")]
    directory: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(long, default_value_t = 1)]
        concurrency: usize,
        /// Optional token-authenticated OpenAI-compatible HTTP data plane (loopback only).
        #[arg(long)]
        listen: Option<std::net::SocketAddr>,
    },
    /// Request files contain an `AdminOperation` or `InferenceRequest`, without an envelope.
    Request {
        file: PathBuf,
        #[arg(long)]
        admin: bool,
    },
    Health,
    Metrics,
    Discover {
        #[arg(default_value = "http://127.0.0.1:11434")]
        candidates: Vec<String>,
    },
    Registry {
        endpoint_id: String,
        #[arg(long)]
        model: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let directory = cli
        .directory
        .or_else(|| {
            std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
                })
                .map(|path| path.join("cerynth/inferenced"))
        })
        .context("set --directory or XDG_STATE_HOME")?;
    let (admin, operation) = match cli.command {
        Command::Serve {
            concurrency,
            listen,
        } => {
            anyhow::ensure!((1..=32).contains(&concurrency), "concurrency must be 1..32");
            return cerynth_inferenced::serve_with_http(directory, concurrency, listen, async {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=term.recv()=>{} }
            })
            .await;
        }
        Command::Request { file, admin } => (admin, serde_json::from_slice(&std::fs::read(file)?)?),
        Command::Health => (true, serde_json::json!({"type":"health"})),
        Command::Metrics => (true, serde_json::json!({"type":"metrics"})),
        Command::Discover { candidates } => (
            true,
            serde_json::json!({"type":"discover","candidates":candidates}),
        ),
        Command::Registry { endpoint_id, model } => (
            true,
            serde_json::json!({"type":"registry","endpoint_id":endpoint_id,"model":model}),
        ),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(
            &cerynth_inferenced::call(&directory, admin, operation).await?
        )?
    );
    Ok(())
}
