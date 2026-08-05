use std::path::Path;

use tokio::net::UnixListener;

use cerynth_ipc::DEFAULT_SOCKET_PATH;

use crate::backend::SharedBackend;

use super::connection::handle_connection;

use super::signals::wait_for_shutdown;

pub async fn start_server(backend: SharedBackend) -> std::io::Result<()> {
    if Path::new(DEFAULT_SOCKET_PATH).exists() {
        std::fs::remove_file(DEFAULT_SOCKET_PATH)?;
    }

    let listener = UnixListener::bind(DEFAULT_SOCKET_PATH)?;

    println!("✓ Cerynth daemon listening on {}", DEFAULT_SOCKET_PATH);

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;

                let backend = backend.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, backend).await {
                        eprintln!("Connection error: {}", e);
                    }
                });
            }

            _ = wait_for_shutdown() => {
                println!("Shutting down daemon...");

                if Path::new(DEFAULT_SOCKET_PATH).exists() {
                    let _ = std::fs::remove_file(DEFAULT_SOCKET_PATH);
                } 

                break;
            }
        }
    }

    Ok(())

}
