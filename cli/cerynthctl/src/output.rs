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
            println!("Adaptation Mode : {:?}", status.adaptation_mode);
            println!(
                "Running         : {}",
                if status.running { "Yes" } else { "No" }
            );
            println!(
                "sched_ext state : {}",
                status.sched_ext_state.as_deref().unwrap_or("not available")
            );
            println!(
                "Heartbeat       : {}",
                if status.heartbeat_ok {
                    "OK"
                } else {
                    "stale/missing"
                }
            );
        }

        SocketResponse::Profile { profile } => {
            println!("Current Profile : {:?}", profile);
        }

        SocketResponse::AdaptationMode { mode } => {
            println!("Adaptation Mode : {:?}", mode);
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
