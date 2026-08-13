mod backend;
mod handlers;
mod paths;
mod server;
mod state;

use std::sync::Arc;

use backend::{MockBackend, SharedBackend};
use cerynth_config::{Config, RuntimeState};
use state::DaemonState;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    println!("Starting Cerynth daemon...\n");

    // Resolved once, in `paths`, so load and save can never disagree.
    let config_path = paths::config_path();
    let state_path = paths::state_path();

    // Load configuration.
    let config = Config::load(config_path);

    // Load persisted runtime state.
    let runtime_state = RuntimeState::load(state_path);

    // Convert persisted state into daemon state.
    let daemon_state: DaemonState = runtime_state.into();

    // Create a shared backend.
    let backend: SharedBackend = Arc::new(Mutex::new(MockBackend::new(daemon_state)));

    println!("Config file          : {config_path}");
    println!("State file           : {state_path}");
    println!("Default profile      : {:?}", config.default_profile);
    println!("Scheduler backend    : {:?}", config.scheduler_backend);
    println!("Scheduler binary     : {}", config.scheduler_binary);
    println!("Adaptation enabled   : {}", config.adaptation_enabled);

    println!();

    server::start_server(backend).await
}
