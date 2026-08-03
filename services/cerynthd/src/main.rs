mod backend;
mod handlers;
mod protocol;
mod server;
mod state;

fn main() {
    println!("Starting Cerynth daemon...\n");

    server::start_server();
}
