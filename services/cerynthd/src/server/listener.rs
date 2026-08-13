use std::os::unix::fs::PermissionsExt;

use tokio::net::UnixListener;


use crate::backend::SharedBackend;

use super::connection::handle_connection;

use super::signals::wait_for_shutdown;

/// Resolves the socket path shared with cerynthctl through cerynth-ipc.
fn socket_path() -> std::path::PathBuf {
    std::path::PathBuf::from(cerynth_ipc::socket_path())
}

pub async fn start_server(backend: SharedBackend) -> std::io::Result<()> {
    let socket_path = socket_path();
    let socket = socket_path.as_path();

    // systemd's RuntimeDirectory= normally creates the parent, but the daemon
    // must also work when started by hand on a fresh boot.
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // A socket file left behind by an unclean shutdown would make bind fail.
    if socket.exists() {
        std::fs::remove_file(socket)?;
    }

    let listener = UnixListener::bind(socket)?;

    // Set the mode explicitly rather than inheriting the umask.
    std::fs::set_permissions(socket, PermissionsExt::from_mode(0o660))?;

    println!("✓ Cerynth daemon listening on {}", socket.display());

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

                if socket.exists() {
                    let _ = std::fs::remove_file(socket);
                }

                break;
            }
        }
    }

    Ok(())
}
