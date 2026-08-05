mod client;
mod commands;
mod output;
mod transport;
mod timeout;
mod error;

use commands::parse_command;
use output::print_response;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let request = match parse_command(&args) {
        Some(req) => req,
        None => {
            println!("Usage:");
            println!("  cerynthctl status");
            println!("  cerynthctl profile get");
            println!("  cerynthctl profile set <balanced|interactive|performance|background>");
            println!("  cerynthctl adaptation pause");
            println!("  cerynthctl adaptation resume");
            return;
        }
    };

    let envelope = match client::execute(request) {
        Ok(response) => response,
        Err(_) => {
            eprintln!("Error: Could not connect to cerynthd.");
            eprintln!("Is the daemon running?");
            std::process::exit(1);
        }
    };

    print_response(envelope);

}
