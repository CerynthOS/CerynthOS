mod backend;
mod handlers;
mod server;
mod state;

use backend::MockBackend;
use cerynth_ipc::{Profile, Request, SchedulerBackend};
use handlers::handle_request;
use state::DaemonState;

fn main() {
    println!("Starting Cerynth daemon...\n");

    let state = DaemonState {
        profile: Profile::Balanced,
        adaptation_enabled: false,
        scheduler_backend: SchedulerBackend::Mock,
    };

    let mut backend = MockBackend::new(state);

    let response = handle_request(
        &mut backend,
        Request::Status,
    );

    println!("{:#?}", response);
}
