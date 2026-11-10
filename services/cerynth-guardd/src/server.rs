//! Local newline-JSON transport. The control socket is accessible only to
//! the daemon's Unix user. Each sandbox receives only its own session socket.
//! A session socket is a bearer capability: never expose it to another agent.
use crate::dispatch::{Caller, dispatch};
use crate::protocol::{Request, Response};
use crate::state::Guard;
use std::collections::HashMap;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;
use uuid::Uuid;

const MAX_FRAME: u64 = 64 * 1024;
type Sessions = Arc<Mutex<HashMap<Uuid, JoinHandle<()>>>>;

fn private_dir(path: &Path) -> anyhow::Result<()> {
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_dir() || m.uid() != nix::unistd::geteuid().as_raw() || m.mode() & 0o077 != 0 {
        anyhow::bail!(
            "{} must be a private directory owned by the service user",
            path.display()
        );
    }
    Ok(())
}

fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    // Never remove an existing path: it could be a live daemon or an attacker
    // supplied object. The operator must investigate stale sockets.
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

async fn write_response(
    stream: &mut tokio::net::unix::OwnedWriteHalf,
    response: &Response,
) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(response)?;
    bytes.push(b'\n');
    stream.write_all(&bytes).await
}

fn connection(
    stream: UnixStream,
    guard: Arc<Guard>,
    caller: Caller,
    sessions: Sessions,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move {
        // UID authentication protects the trusted endpoint. Session endpoints are
        // additionally confined by the sandbox's mount namespace.
        if stream
            .peer_cred()
            .ok()
            .is_none_or(|c| c.uid() != nix::unistd::geteuid().as_raw())
        {
            return;
        }
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        loop {
            if let Caller::Session(id) = caller {
                if !guard.has_session(id) {
                    break;
                }
            }
            let mut line = Vec::new();
            let n = match (&mut reader)
                .take(MAX_FRAME + 1)
                .read_until(b'\n', &mut line)
                .await
            {
                Ok(n) => n,
                Err(_) => break,
            };
            if n == 0 {
                break;
            }
            if n as u64 > MAX_FRAME || line.last() != Some(&b'\n') {
                break;
            }
            let req: Request = match serde_json::from_slice(&line) {
                Ok(v) => v,
                Err(_) => {
                    if write_response(
                        &mut writer,
                        &Response::error("invalid_request", "invalid request JSON"),
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                    continue;
                }
            };
            if matches!(req, Request::Subscribe) {
                if caller != Caller::Trusted {
                    let _ = write_response(
                        &mut writer,
                        &Response::error(
                            "caller_not_authorized",
                            "subscription requires trusted caller",
                        ),
                    )
                    .await;
                    break;
                }
                let mut rx = guard.events.subscribe();
                if write_response(&mut writer, &Response::Ok).await.is_err() {
                    break;
                }
                while let Ok(event) = rx.recv().await {
                    if write_response(&mut writer, &Response::Event { event })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                break;
            }
            if let (Caller::Trusted, Request::RegisterSession { registration }) = (caller, &req) {
                let id = registration.session.session_id;
                // Bind first; if registration fails, discard the socket.
                let path = guard.session_socket(id);
                let listener = match bind(&path) {
                    Ok(l) => l,
                    Err(_) => {
                        let _ = write_response(
                            &mut writer,
                            &Response::error("socket_unavailable", "cannot create session socket"),
                        )
                        .await;
                        continue;
                    }
                };
                let egress_path = guard
                    .cfg
                    .runtime_dir
                    .join("sessions")
                    .join(format!("{id}.egress.sock"));
                let egress = match bind(&egress_path) {
                    Ok(l) => l,
                    Err(_) => {
                        drop(listener);
                        let _ = std::fs::remove_file(&path);
                        let _ = write_response(
                            &mut writer,
                            &Response::error("socket_unavailable", "cannot create egress socket"),
                        )
                        .await;
                        continue;
                    }
                };
                let response = dispatch(&guard, caller, req).await;
                if matches!(response, Response::Ok) {
                    let g = guard.clone();
                    let s = sessions.clone();
                    let task = tokio::spawn(async move {
                        tokio::select! {
                            () = accept_loop(listener, g.clone(), Caller::Session(id), s) => {},
                            _ = crate::egress::serve(egress, g, id) => {},
                        }
                    });
                    sessions.lock().unwrap().insert(id, task);
                } else {
                    drop(listener);
                    drop(egress);
                    let _ = std::fs::remove_file(&egress_path);
                    let _ = std::fs::remove_file(&path);
                }
                if write_response(&mut writer, &response).await.is_err() {
                    break;
                }
                continue;
            }
            let ended = match (&caller, &req) {
                (Caller::Trusted, Request::EndSession { session_id }) => Some(*session_id),
                _ => None,
            };
            let response = dispatch(&guard, caller, req).await;
            if matches!(response, Response::Ok) {
                if let Some(id) = ended {
                    if let Some(task) = sessions.lock().unwrap().remove(&id) {
                        task.abort();
                    }
                }
            }
            if write_response(&mut writer, &response).await.is_err() {
                break;
            }
        }
    })
}

async fn accept_loop(
    listener: UnixListener,
    guard: Arc<Guard>,
    caller: Caller,
    sessions: Sessions,
) {
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(connection(stream, guard.clone(), caller, sessions.clone()));
    }
}

/// Run until Ctrl-C. Existing socket paths cause startup failure.
pub async fn serve(guard: Arc<Guard>) -> anyhow::Result<()> {
    private_dir(&guard.cfg.runtime_dir)?;
    private_dir(&guard.cfg.runtime_dir.join("sessions"))?;
    let path: PathBuf = guard.cfg.runtime_dir.join("guard.sock");
    let listener = bind(&path)?;
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        () = accept_loop(listener, guard.clone(), Caller::Trusted, sessions.clone()) => {},
        result = tokio::signal::ctrl_c() => { result?; },
        _ = sigterm.recv() => {},
    }
    for (id, task) in sessions.lock().unwrap().drain() {
        task.abort();
        let _ = std::fs::remove_file(
            guard
                .cfg
                .runtime_dir
                .join("sessions")
                .join(format!("{id}.egress.sock")),
        );
        let p = guard.session_socket(id);
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_socket()) {
            let _ = std::fs::remove_file(p);
        }
    }
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_socket()) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GuardConfig;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    async fn call(path: &Path, payload: serde_json::Value) -> serde_json::Value {
        let mut stream = UnixStream::connect(path).await.unwrap();
        let mut text = payload.to_string();
        text.push('\n');
        stream.write_all(text.as_bytes()).await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        reader.read_line(&mut response).await.unwrap();
        serde_json::from_str(&response).unwrap()
    }

    #[tokio::test]
    async fn session_socket_cannot_use_trusted_operations_or_impersonate() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = GuardConfig {
            runtime_dir: dir.path().join("run"),
            state_dir: dir.path().join("state"),
            ..Default::default()
        };
        let guard = cfg.build().unwrap();
        private_dir(&cfg.runtime_dir).unwrap();
        private_dir(&cfg.runtime_dir.join("sessions")).unwrap();
        let control = cfg.runtime_dir.join("guard.sock");
        let listener = bind(&control).unwrap();
        let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
        let runner = tokio::spawn(accept_loop(
            listener,
            guard.clone(),
            Caller::Trusted,
            sessions.clone(),
        ));
        let id = Uuid::new_v4();
        let registration = serde_json::json!({"op":"register_session", "registration": {"session_id": id, "workspace": dir.path(), "capabilities": []}});
        assert_eq!(call(&control, registration).await["type"], "ok");
        let socket = guard.session_socket(id);
        let denied = call(&socket, serde_json::json!({"op":"pending_approvals"})).await;
        assert_eq!(denied["code"], "caller_not_authorized");
        let request = cerynth_security::ActionRequest::new(
            Uuid::new_v4(),
            cerynth_security::CapabilityKind::FsRead,
            "workspace",
            serde_json::json!({}),
        );
        let denied = call(
            &socket,
            serde_json::json!({"op":"decide", "request":request}),
        )
        .await;
        assert_eq!(denied["decision"]["reason"], "caller_not_authorized");
        assert_eq!(
            call(
                &control,
                serde_json::json!({"op":"end_session", "session_id":id})
            )
            .await["type"],
            "ok"
        );
        assert!(!socket.exists());
        runner.abort();
    }

    #[test]
    fn rejects_insecure_runtime_directory_and_existing_socket() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_dir(dir.path()).is_err());
        let p = dir.path().join("guard.sock");
        std::fs::write(&p, "do not remove").unwrap();
        assert!(bind(&p).is_err());
        assert_eq!(std::fs::read_to_string(p).unwrap(), "do not remove");
    }
}
