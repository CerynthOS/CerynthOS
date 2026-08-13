mod backend;
mod backends;
mod handlers;
mod paths;
mod server;
mod state;

use std::path::PathBuf;
use std::sync::Arc;

use backend::SharedBackend;
use backends::ScxBackend;
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

    // Resolve the scheduler binary: an explicit env override wins, otherwise
    // fall back to the configured path (default /usr/bin/cerynth-scx).
    let scheduler_binary = std::env::var_os("CERYNTH_SCX_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| config.scheduler_binary.clone());

    println!("Config file          : {config_path}");
    println!("State file           : {state_path}");
    println!("Default profile      : {:?}", config.default_profile);
    println!("Scheduler backend    : {:?}", config.scheduler_backend);
    println!("Scheduler binary     : {}", config.scheduler_binary.display());
    println!("Adaptation enabled   : {}", config.adaptation_enabled);
    println!("Scheduler binary     : {}", scheduler_binary.display());
    println!("Auto-start           : {}", config.auto_start);

    // Create a shared SCX backend.
    let backend: SharedBackend = Arc::new(Mutex::new(Box::new(
        ScxBackend::new(scheduler_binary, daemon_state.profile.clone())
            .with_adaptation(daemon_state.adaptation_enabled),
    )));

    // Auto-start the scheduler unless disabled. A failure is non-fatal: the
    // scheduler can still be started later via `cerynthctl start`.
    if config.auto_start {
        match backend.lock().await.start() {
            Ok(()) => println!("✓ Scheduler auto-started"),
            Err(e) => eprintln!("Warning: failed to auto-start scheduler: {e}"),
        }
    } else {
        println!("Scheduler auto-start disabled; waiting for an explicit start");
    }

    println!();

    server::start_server(backend).await
}
