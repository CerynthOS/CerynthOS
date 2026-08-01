mod commands;

use commands::parse_command;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    println!("Starting cerynthctl...\n");

    match parse_command(&args) {
        Some(request) => {
            println!("Sending request:\n{:#?}", request);
        }

        None => {
            println!("Usage:");
            println!("  cerynthctl status");
            println!("  cerynthctl profile get");
            println!("  cerynthctl profile set <balanced|interactive|performance|background>");
            println!("  cerynthctl adaptation pause");
            println!("  cerynthctl adaptation resume");
        }
    }
}
