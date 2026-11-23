use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::sync::Arc;

use tokio::net::UnixListener;

use crate::backend::SharedBackend;

use super::connection::handle_connection;

use super::signals::wait_for_shutdown;

/// Resolves the socket path shared with cerynthctl through cerynth-ipc.
fn socket_path() -> std::path::PathBuf {
    std::path::PathBuf::from(cerynth_ipc::socket_path())
}

pub async fn start_server(backend: SharedBackend, auto_start: bool) -> std::io::Result<()> {
    let socket_path = socket_path();
    let socket = socket_path.as_path();

    // systemd's RuntimeDirectory= normally creates the parent, but the daemon
    // must also work when started by hand on a fresh boot.
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Hold a non-following, same-owner lock through shutdown. Never unlink a
    // live daemon's endpoint or an unrelated file supplied as a socket path.
    let lock_path = socket_path.with_extension("sock.lock");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock_path)?;
    // SAFETY: geteuid takes no arguments and has no memory preconditions.
    let owner = unsafe { libc::geteuid() };
    if lock.metadata()?.uid() != owner {
        return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    }
    // SAFETY: lock is a live file descriptor, held until this function returns.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    match std::fs::symlink_metadata(socket) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() || metadata.uid() != owner {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "refusing to replace a non-owned socket path",
                ));
            }
            match tokio::time::timeout(
                std::time::Duration::from_secs(1),
                tokio::net::UnixStream::connect(socket),
            )
            .await
            {
                Ok(Ok(_)) | Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AddrInUse,
                        "daemon endpoint is active",
                    ));
                }
                Ok(Err(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    std::fs::remove_file(socket)?
                }
                Ok(Err(e)) => return Err(e),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let listener = UnixListener::bind(socket)?;

    // Set the mode explicitly rather than inheriting the umask.
    std::fs::set_permissions(socket, PermissionsExt::from_mode(0o660))?;

    let shutdown = wait_for_shutdown()?;
    tokio::pin!(shutdown);
    // Establish endpoint ownership and signal handling before creating a child.
    if auto_start {
        match backend.lock().await.start() {
            Ok(()) => println!("✓ Scheduler auto-started"),
            Err(error) => eprintln!("Warning: failed to auto-start scheduler: {error}"),
        }
    }

    println!("✓ Cerynth daemon listening on {}", socket.display());
    let capacity = Arc::new(tokio::sync::Semaphore::new(32));

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;
                let credentials = stream.peer_cred()?;
                if credentials.uid() != owner && credentials.uid() != 0 { continue; }
                let Ok(permit) = capacity.clone().try_acquire_owned() else { continue; };

                let backend = backend.clone();

                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(e) = handle_connection(stream, backend).await {
                        eprintln!("Connection error: {e}");
                    }
                });
            }

            _ = &mut shutdown => {
                println!("Shutting down daemon...");
                let mut backend = backend.lock().await;
                if backend.status().is_ok_and(|status| status.running) {
                    if let Err(error) = backend.stop() { eprintln!("Scheduler shutdown failed: {error}"); }
                }

                if socket.exists() {
                    let _ = std::fs::remove_file(socket);
                }

                break;
            }
        }
    }

    Ok(())
}
