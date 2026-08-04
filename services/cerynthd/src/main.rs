mod backend;
mod handlers;
mod server;
mod state;

use backend::MockBackend;
use cerynth_config::{Config, RuntimeState};
use state::DaemonState;

const CONFIG_PATH: &str = "/etc/cerynth/cerynth.toml";
const STATE_PATH: &str = "/var/lib/cerynth/state.json";

#[tokio::main]
async fn main() -> std::io::Result<()> {
    println!("Starting Cerynth daemon...\n");

    let config = Config::load(CONFIG_PATH);

    let runtime_state = RuntimeState::load(STATE_PATH);

    let daemon_state: DaemonState = runtime_state.into();

    let _backend = MockBackend::new(daemon_state);

    println!("Default profile      : {:?}", config.default_profile);
    println!("Scheduler backend    : {:?}", config.scheduler_backend);
    println!("Adaptation enabled   : {}", config.adaptation_enabled);

    println!();

    server::start_server().await
}
