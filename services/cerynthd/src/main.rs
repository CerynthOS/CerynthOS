mod backend;
mod backends;
mod handlers;
mod server;
mod state;

use std::path::PathBuf;
use std::sync::Arc;

use backend::SharedBackend;
use backends::ScxBackend;
use cerynth_config::{Config, RuntimeState};
use state::DaemonState;
use tokio::sync::Mutex;

const CONFIG_PATH: &str = "/etc/cerynth/cerynth.toml";
const STATE_PATH: &str = "runtime_state.json";

/// Absolute path to the daemon's persisted runtime-state file.
///
/// Overridable via `CERYNTH_STATE_PATH` so that each integration test can run
/// against an isolated state file instead of sharing the default one. Falls
/// back to `STATE_PATH` (in the daemon's working directory) when unset.
pub fn state_path() -> String {
    std::env::var_os("CERYNTH_STATE_PATH")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| STATE_PATH.to_string())
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    println!("Starting Cerynth daemon...\n");

    // Load configuration.
    let config = Config::load(CONFIG_PATH);

    // Load persisted runtime state.
    let runtime_state = RuntimeState::load(&state_path());

    // Convert persisted state into daemon state.
    let daemon_state: DaemonState = runtime_state.into();

    // Resolve the scheduler binary: an explicit env override wins, otherwise
    // fall back to the configured path (default /usr/bin/cerynth-scx).
    let scheduler_binary = std::env::var_os("CERYNTH_SCX_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| config.scheduler_binary.clone());

    println!("Default profile      : {:?}", config.default_profile);
    println!("Scheduler backend    : {:?}", config.scheduler_backend);
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
