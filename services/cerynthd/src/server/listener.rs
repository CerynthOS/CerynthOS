use std::path::Path;

use tokio::net::UnixListener;

use cerynth_ipc::DEFAULT_SOCKET_PATH;

use crate::backend::SharedBackend;

use super::connection::handle_connection;

use super::signals::wait_for_shutdown;

/// Resolves the socket path the daemon binds.
///
/// Overridable via `CERYNTH_SOCKET_PATH` so each integration test can run its
/// own daemon on an isolated socket instead of contending for the default one.
/// Falls back to the protocol default when unset.
fn socket_path() -> std::path::PathBuf {
    std::env::var_os("CERYNTH_SOCKET_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(DEFAULT_SOCKET_PATH))
}

pub async fn start_server(backend: SharedBackend) -> std::io::Result<()> {
    let socket_path = socket_path();

    if Path::new(&socket_path).exists() {
        std::fs::remove_file(&socket_path)?;
    }

    let listener = UnixListener::bind(&socket_path)?;

    println!("✓ Cerynth daemon listening on {}", socket_path.display());

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

                if Path::new(&socket_path).exists() {
                    let _ = std::fs::remove_file(&socket_path);
                }

                break;
            }
        }
    }

    Ok(())
}
