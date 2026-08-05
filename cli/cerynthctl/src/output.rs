use cerynth_ipc::{ResponseEnvelope, SocketResponse};

pub fn print_response(envelope: ResponseEnvelope) {
    match envelope.response {
        SocketResponse::Status { status } => {
            println!("Current Profile : {:?}", status.profile);
            println!("Backend         : {:?}", status.backend);
            println!(
                "Adaptation      : {}",
                if status.adaptation_enabled {
                    "Enabled"
                } else {
                    "Disabled"
                }
            );
        }

        SocketResponse::Profile { profile } => {
            println!("Current Profile : {:?}", profile);
        }

        SocketResponse::Success => {
            println!("✓ Success");
        }

        SocketResponse::Error { message } => {
            eprintln!("✗ {}", message);
        }

        SocketResponse::Pong => {
            println!("Pong");
        }
    }
}
