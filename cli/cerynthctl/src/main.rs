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
        std::process::exit(2);
    };

    let envelope = match client::execute(request) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("Error: request failed: {error}");
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) {
                eprintln!("The operation may have been accepted; inspect status before retrying.");
            }
            std::process::exit(1);
        }
    };

    let failed = matches!(envelope.response, cerynth_ipc::SocketResponse::Error { .. });
    print_response(envelope);
    if failed {
        std::process::exit(1);
    }
}
