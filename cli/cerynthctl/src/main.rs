mod client;
mod commands;
mod error;
mod output;
mod timeout;
mod transport;

use commands::parse_command;
use output::print_response;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let request = if let Some(req) = parse_command(&args) {
        req
    } else {
        println!("Usage:");
        println!("  cerynthctl status");
        println!("  cerynthctl start");
        println!("  cerynthctl stop");
        println!("  cerynthctl restart");
        println!("  cerynthctl profile get");
        println!("  cerynthctl profile set <balanced|interactive|performance|background>");
        println!("  cerynthctl adaptation pause");
        println!("  cerynthctl adaptation resume");
        println!("  cerynthctl adaptation mode get");
        println!("  cerynthctl adaptation mode set <off|shadow|canary>");
        return;
    };

    let envelope = if let Ok(response) = client::execute(request) {
        response
    } else {
        eprintln!("Error: Could not connect to cerynthd.");
        eprintln!("Is the daemon running?");
        std::process::exit(1);
    };

    print_response(envelope);
}
