mod backend;
mod backends;
mod handlers;
mod server;
mod state;

use std::path::PathBuf;
use std::sync::Arc;

use backend::SharedBackend;
use backends::{MockBackend, ScxBackend};
use cerynth_config::{Config, RuntimeState};
use cerynth_ipc::SchedulerBackend;
use state::DaemonState;
use tokio::sync::Mutex;

const DEFAULT_CONFIG_PATH: &str = "/etc/cerynth/cerynth.toml";
const STATE_PATH: &str = "runtime_state.json";

pub fn state_path() -> String {
    std::env::var_os("CERYNTH_STATE_PATH")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| STATE_PATH.to_string())
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    println!("Starting Cerynth daemon...\n");

    let config_path = std::env::var("CERYNTH_CONFIG_PATH")
        .unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());

    let config = Config::load(&config_path);
    let runtime_state = RuntimeState::load(&state_path());

    let daemon_state: DaemonState = runtime_state.into();

    let scheduler_binary = std::env::var_os("CERYNTH_SCX_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| config.scheduler_binary.clone());

    println!("Default profile      : {:?}", config.default_profile);
    println!("Scheduler backend    : {:?}", config.scheduler_backend);
    println!("Adaptation enabled   : {}", config.adaptation_enabled);
    println!("Scheduler binary     : {}", scheduler_binary.display());
    println!("Auto-start           : {}", config.auto_start);

    let backend: SharedBackend = match config.scheduler_backend {
        SchedulerBackend::Mock => {
            println!("Using MockBackend");

            Arc::new(Mutex::new(Box::new(
                MockBackend::new(daemon_state),
            )))
        }

        SchedulerBackend::Scx => {
            println!("Using ScxBackend");

            Arc::new(Mutex::new(Box::new(
                ScxBackend::new(
                    scheduler_binary,
                    daemon_state.profile.clone(),
                )
                .with_adaptation(daemon_state.adaptation_enabled),
            )))
        }
    };

    if config.auto_start {
        match backend.lock().await.start() {
            Ok(()) => println!("✓ Scheduler auto-started"),

            Err(e) => {
                eprintln!(
                    "Warning: failed to auto-start scheduler: {e}"
                );
            }
        }
    } else {
        println!(
            "Scheduler auto-start disabled; \
             waiting for an explicit start"
        );
    }

    println!();

    server::start_server(backend).await
}
