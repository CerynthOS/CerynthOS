mod backend;
mod handlers;
mod server;
mod state;

use std::sync::Arc;

use backend::{MockBackend, SharedBackend};
use cerynth_config::{Config, RuntimeState};
use state::DaemonState;
use tokio::sync::Mutex;

const CONFIG_PATH: &str = "/etc/cerynth/cerynth.toml";
const STATE_PATH: &str = "runtime_state.json";

#[tokio::main]
async fn main() -> std::io::Result<()> {
    println!("Starting Cerynth daemon...\n");

    // Load configuration.
    let config = Config::load(CONFIG_PATH);

    // Load persisted runtime state.
    let runtime_state = RuntimeState::load(STATE_PATH);

    // Convert persisted state into daemon state.
    let daemon_state: DaemonState = runtime_state.into();

    // Create a shared backend.
    let backend: SharedBackend = Arc::new(Mutex::new(MockBackend::new(daemon_state)));

    println!("Default profile      : {:?}", config.default_profile);
    println!("Scheduler backend    : {:?}", config.scheduler_backend);
    println!("Adaptation enabled   : {}", config.adaptation_enabled);

    println!();

    server::start_server(backend).await
}
