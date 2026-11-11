use std::path::PathBuf;

use anyhow::{Context, Result};
use cerynth_agent_api::{LaunchRequest, ModelConfig, Operation, SessionSpec};
use clap::{Parser, Subcommand};
use uuid::Uuid;

#[derive(Parser)]
#[command(about = "Unprivileged Cerynth agent registry and local inference prototype")]
struct Cli {
    /// Private state/socket directory; defaults to `$XDG_STATE_HOME/cerynth/agentd`.
    #[arg(long, global = true, env = "CERYNTH_AGENT_DIR")]
    directory: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve {
        /// Ollama origin. Configure Ollama with `OLLAMA_NO_CLOUD=1` for local-only operation.
        #[arg(long, default_value = "http://127.0.0.1:11434")]
        endpoint: String,
        /// Permit explicitly requested unisolated sessions (development only).
        #[arg(long)]
        allow_unisolated: bool,
    },
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        model: String,
        #[arg(long, default_value = "inference_probe")]
        harness: String,
    },
    CreateSpec {
        file: PathBuf,
    },
    Start {
        id: Uuid,
        #[arg(long)]
        operation_id: Option<Uuid>,
        #[arg(long)]
        pty: bool,
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },
    HarnessStart {
        id: Uuid,
        #[arg(long)]
        operation_id: Option<Uuid>,
    },
    Input {
        id: Uuid,
    },
    Stop {
        id: Uuid,
    },
    Cleanup {
        id: Uuid,
    },
    Output {
        id: Uuid,
        #[arg(long, default_value_t = 0)]
        offset: u64,
    },
    Events {
        id: Uuid,
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
    Processes {
        id: Uuid,
    },
    Worktree {
        id: Uuid,
    },
    Diagnostics,
    /// Advanced versioned operations, such as delegation or artifact collection.
    Request {
        file: PathBuf,
    },
    List,
    Status {
        id: Uuid,
    },
    /// Read the prompt from stdin so it does not enter shell history/process arguments.
    Infer {
        id: Uuid,
    },
    /// Cancels the client request; this does not guarantee server/GPU preemption.
    Cancel {
        id: Uuid,
    },
    Models,
    Adapters,
    Discover {
        #[arg(long, default_value = "open_code")]
        harness: String,
    },
}

async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
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
                .map(|path| path.join("cerynth/agentd"))
        })
        .context("set --directory or XDG_STATE_HOME")?;
    let operation = match cli.command {
        Command::Serve {
            endpoint,
            allow_unisolated,
        } => {
            return cerynth_agentd::serve_with_policy(
                directory,
                &endpoint,
                allow_unisolated,
                shutdown(),
            )
            .await;
        }
        Command::Create {
            name,
            workspace,
            model,
            harness,
        } => Operation::Create {
            spec: SessionSpec {
                name,
                harness: serde_json::from_value(serde_json::Value::String(harness))?,
                workspace: workspace.canonicalize().context("invalid workspace")?,
                model: ModelConfig {
                    name: model,
                    mode: cerynth_agent_api::ModelMode::Local,
                    endpoint_id: None,
                    cloud_consent: false,
                    disable_thinking: false,
                },
                runtime: cerynth_agent_api::RuntimeSpec::default(),
            },
        },
        Command::List => Operation::List,
        Command::Status { id } => Operation::Status { id },
        Command::Cancel { id } => Operation::Cancel { id },
        Command::Models => Operation::Models,
        Command::Adapters => Operation::Adapters,
        Command::Discover { harness } => Operation::DiscoverHarness {
            harness: serde_json::from_value(serde_json::Value::String(harness))?,
        },
        Command::CreateSpec { file } => Operation::Create {
            spec: serde_json::from_slice(&std::fs::read(file)?)?,
        },
        Command::Request { file } => serde_json::from_slice(&std::fs::read(file)?)?,
        Command::Start {
            id,
            operation_id,
            pty,
            command,
        } => Operation::Start {
            id,
            operation_id: operation_id.unwrap_or_else(Uuid::new_v4),
            launch: LaunchRequest {
                program: PathBuf::from(&command[0]),
                args: command[1..].to_vec(),
                initial_input: None,
                pty,
            },
        },
        Command::HarnessStart { id, operation_id } => Operation::HarnessStart {
            id,
            operation_id: operation_id.unwrap_or_else(Uuid::new_v4),
            input: read_input().await?,
        },
        Command::Input { id } => Operation::Input {
            id,
            input: read_input().await?,
        },
        Command::Stop { id } => Operation::Stop { id },
        Command::Cleanup { id } => Operation::Cleanup { id },
        Command::Output { id, offset } => Operation::Output {
            id,
            offset,
            limit: 32768,
        },
        Command::Events { id, after } => Operation::Events { id, after },
        Command::Processes { id } => Operation::ProcessTree { id },
        Command::Worktree { id } => Operation::Worktree { id },
        Command::Diagnostics => Operation::Diagnostics,
        Command::Infer { id } => Operation::Infer {
            id,
            prompt: read_input().await?,
        },
    };
    let output = cerynth_agentd::call(&directory, operation).await?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

async fn read_input() -> Result<String> {
    use tokio::io::AsyncReadExt;
    let mut input = String::new();
    tokio::io::stdin()
        .take(cerynth_agent_api::MAX_PROMPT_BYTES as u64 + 1)
        .read_to_string(&mut input)
        .await?;
    Ok(input)
}
