mod commands;

use commands::parse_command;
use cerynth_ipc::Request;
use serde_json::json;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

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

    let json_request = match request {

        Request::Status =>
            json!({"command":"status"}),

        Request::GetProfile =>
            json!({"command":"get-profile"}),

        Request::PauseAdaptation =>
            json!({"command":"pause-adaptation"}),

        Request::ResumeAdaptation =>
            json!({"command":"resume-adaptation"}),

        Request::SetProfile(profile) => {

            let profile = format!("{:?}", profile).to_lowercase();

            json!({
                "command":"set-profile",
                "profile":profile
            })
        }
    };

    let mut stream =
        UnixStream::connect("/tmp/cerynthd.sock")
            .expect("Failed to connect to daemon");

    writeln!(
        stream,
        "{}",
        serde_json::to_string(&json_request).unwrap()
    )
    .unwrap();

    let mut reader = BufReader::new(stream);

    let mut response = String::new();

    reader.read_line(&mut response).unwrap();

    println!("{}", response);
}
