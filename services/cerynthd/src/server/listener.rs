use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tokio::net::UnixListener;

use cerynth_ipc::socket_path;

use crate::backend::SharedBackend;

use super::connection::handle_connection;

use super::signals::wait_for_shutdown;

pub async fn start_server(backend: SharedBackend) -> std::io::Result<()> {
    let socket = socket_path();
    let socket = socket.as_str();

    // systemd's RuntimeDirectory= normally creates the parent, but the daemon
    // must also work when started by hand on a fresh boot.
    if let Some(parent) = Path::new(socket).parent() {
        std::fs::create_dir_all(parent)?;
    }

    // A socket file left behind by an unclean shutdown would make bind fail.
    if Path::new(socket).exists() {
        std::fs::remove_file(socket)?;
    }

    let listener = UnixListener::bind(socket)?;

    // Set the mode explicitly rather than inheriting the umask, which would
    // otherwise decide who can reach the control plane.
    //
    // Connecting to a Unix socket requires *write* permission, so the 0755
    // that a default umask produces is misleading: it looks world-accessible
    // but only root can actually connect. 0660 states the intent honestly.
    // The daemon controls kernel scheduling, so access stays privileged; see
    // docs/contracts/runtime-v1.md for the group-access follow-up.
    std::fs::set_permissions(socket, PermissionsExt::from_mode(0o660))?;

    println!("✓ Cerynth daemon listening on {socket}");

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

                if Path::new(socket).exists() {
                    let _ = std::fs::remove_file(socket);
                }

                break;
            }
        }
    }

    Ok(())
}
