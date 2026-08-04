use std::path::Path;

use cerynth_ipc::{DEFAULT_SOCKET_PATH, MAX_MESSAGE_SIZE};
use tokio::io::AsyncReadExt;
use tokio::net::{UnixListener, UnixStream};

/// Starts the IPC server.
pub async fn start_server() -> std::io::Result<()> {
    if Path::new(DEFAULT_SOCKET_PATH).exists() {
        std::fs::remove_file(DEFAULT_SOCKET_PATH)?;
    }

    let listener = UnixListener::bind(DEFAULT_SOCKET_PATH)?;

    println!("✓ Cerynth daemon listening on {}", DEFAULT_SOCKET_PATH);

    loop {
        let (stream, _) = listener.accept().await?;
        println!("Client connected.");

        if let Err(e) = handle_connection(stream).await {
            eprintln!("Connection error: {}", e);
        }
    }
}

async fn handle_connection(
    mut stream: UnixStream,
) -> std::io::Result<()> {
    let mut buffer = vec![0u8; MAX_MESSAGE_SIZE];

    let bytes_read = stream.read(&mut buffer).await?;

    if bytes_read == 0 {
        return Ok(());
    }

    let message = String::from_utf8_lossy(&buffer[..bytes_read]);

    println!("Received:");

    println!("{}", message.trim());

    Ok(())
}
