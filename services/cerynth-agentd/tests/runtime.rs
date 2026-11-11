use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use cerynth_agent_api::{
    API_VERSION, HarnessAdapter, HarnessKind, MAX_FRAME_BYTES, ModelConfig, Operation, Request,
    Response, Session, SessionSpec, SessionState,
};
use cerynth_agentd::{call, serve};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, UnixStream},
    sync::{Semaphore, oneshot},
    task::JoinHandle,
};

struct Backend {
    endpoint: String,
    requests: Arc<AtomicUsize>,
    release: Arc<Semaphore>,
    task: JoinHandle<()>,
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Backend {
    async fn start(reply: &'static str) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let requests = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Semaphore::new(0));
        let counter = requests.clone();
        let gate = release.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        let counter = counter.clone();
                        let gate = gate.clone();
                        connections.spawn(async move {
                            let mut reader = BufReader::new(stream);
                            let mut line = String::new();
                            reader.read_line(&mut line).await.unwrap();
                            let chat = line.starts_with("POST /api/chat ");
                            assert!(chat || line.starts_with("GET /api/tags "));
                            let mut length = 0;
                            loop {
                                line.clear();
                                reader.read_line(&mut line).await.unwrap();
                                if line == "\r\n" { break; }
                                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                                    length = value.trim().parse::<usize>().unwrap();
                                }
                            }
                            let mut body = vec![0; length];
                            reader.read_exact(&mut body).await.unwrap();
                            if chat {
                                let body: Value = serde_json::from_slice(&body).unwrap();
                                assert_eq!(body["stream"], false);
                                assert_eq!(body["model"], "test-model");
                                assert!(body.get("tools").is_none());
                                counter.fetch_add(1, Ordering::SeqCst);
                                gate.acquire().await.unwrap().forget();
                            }
                            let body = if chat { reply } else {
                                r#"{"models":[{"name":"test-model","size":1024,"digest":"abc"}]}"#
                            };
                            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                            let _ = reader.get_mut().write_all(response.as_bytes()).await;
                        });
                    }
                    Some(result) = connections.join_next(), if !connections.is_empty() => {
                        result.unwrap();
                    }
                }
            }
        });
        Ok(Self {
            endpoint,
            requests,
            release,
            task,
        })
    }

    async fn wait_requests(&self, count: usize) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.requests.load(Ordering::SeqCst) < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .context("backend did not receive inference request")
    }
}

const SUCCESS: &str = r#"{"model":"test-model","message":{"role":"assistant","content":"private response"},"done":true,"eval_count":3}"#;

struct Daemon {
    dir: tempfile::TempDir,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<()>>>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Daemon {
    async fn start(endpoint: &str) -> Result<Self> {
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?;
        let (stop, stopped) = oneshot::channel();
        let path = dir.path().to_path_buf();
        let endpoint = endpoint.to_owned();
        let task = tokio::spawn(async move {
            serve(path, &endpoint, async {
                let _ = stopped.await;
            })
            .await
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            while !dir.path().join("agentd.sock").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .context("daemon failed to bind")?;
        Ok(Self {
            dir,
            stop: Some(stop),
            task: Some(task),
        })
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    async fn create(&self) -> Result<Session> {
        Ok(serde_json::from_value(
            call(
                self.path(),
                Operation::Create {
                    spec: SessionSpec {
                        runtime: cerynth_agent_api::RuntimeSpec::default(),
                        name: "probe".into(),
                        harness: HarnessKind::InferenceProbe,
                        workspace: self.path().into(),
                        model: ModelConfig {
                            name: "test-model".into(),
                            mode: cerynth_agent_api::ModelMode::Local,
                            endpoint_id: None,
                            cloud_consent: false,
                            disable_thinking: false,
                        },
                    },
                },
            )
            .await?,
        )?)
    }

    async fn state(&self, id: uuid::Uuid) -> Result<SessionState> {
        let session: Session =
            serde_json::from_value(call(self.path(), Operation::Status { id }).await?)?;
        Ok(session.state)
    }

    async fn wait_state(&self, id: uuid::Uuid, state: SessionState) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if self.state(id).await? == state {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await??;
        Ok(())
    }

    async fn finish(&mut self) -> Result<()> {
        self.stop.take().unwrap().send(()).ok();
        tokio::time::timeout(Duration::from_secs(3), self.task.take().unwrap()).await???;
        assert!(!self.path().join("agentd.sock").exists());
        Ok(())
    }
}

fn infer(path: PathBuf, id: uuid::Uuid) -> JoinHandle<Result<Value>> {
    tokio::spawn(async move {
        call(
            &path,
            Operation::Infer {
                id,
                prompt: "private prompt".into(),
            },
        )
        .await
    })
}

#[tokio::test]
async fn full_ipc_lifecycle_cli_persistence_and_no_prompt_logging() -> Result<()> {
    let backend = Backend::start(SUCCESS).await?;
    backend.release.add_permits(1);
    let mut daemon = Daemon::start(&backend.endpoint).await?;
    let models = call(daemon.path(), Operation::Models).await?;
    assert_eq!(models[0]["name"], "test-model");
    let session = daemon.create().await?;
    assert_eq!(
        std::fs::metadata(daemon.path().join("agentd.sock"))?.mode() & 0o777,
        0o600
    );
    let output = infer(daemon.path().into(), session.id).await??;
    assert_eq!(output["message"]["content"], "private response");
    assert_eq!(daemon.state(session.id).await?, SessionState::Completed);
    assert!(infer(daemon.path().into(), session.id).await?.is_err());
    assert!(
        call(daemon.path(), Operation::Cancel { id: session.id })
            .await
            .is_err()
    );
    let status = tokio::process::Command::new(env!("CARGO_BIN_EXE_cerynth-agentd"))
        .arg("--directory")
        .arg(daemon.path())
        .arg("status")
        .arg(session.id.to_string())
        .output()
        .await?;
    assert!(status.status.success());
    let status: Session = serde_json::from_slice(&status.stdout)?;
    assert_eq!(
        status.events.last().unwrap().reason,
        "inference_probe_completed"
    );
    let state = std::fs::read_to_string(daemon.path().join("sessions.json"))?;
    assert!(!state.contains("private prompt"));
    assert!(!state.contains("private response"));
    daemon.finish().await?;
    let reopened = cerynth_agentd::AgentService::open(daemon.path(), &backend.endpoint)?;
    drop(reopened);
    Ok(())
}

#[tokio::test]
async fn queued_requests_cancel_without_entering_backend_and_status_stays_responsive() -> Result<()>
{
    let backend = Backend::start(SUCCESS).await?;
    let mut daemon = Daemon::start(&backend.endpoint).await?;
    let first = daemon.create().await?;
    let second = daemon.create().await?;
    let first_call = infer(daemon.path().into(), first.id);
    backend.wait_requests(1).await?;
    let second_call = infer(daemon.path().into(), second.id);
    daemon
        .wait_state(second.id, SessionState::WaitingForInference)
        .await?;
    assert_eq!(backend.requests.load(Ordering::SeqCst), 1);
    call(daemon.path(), Operation::Cancel { id: second.id }).await?;
    assert!(second_call.await?.is_err());
    backend.release.add_permits(1);
    first_call.await??;
    assert_eq!(backend.requests.load(Ordering::SeqCst), 1);
    assert_eq!(daemon.state(second.id).await?, SessionState::Cancelled);
    daemon.finish().await?;
    Ok(())
}

#[tokio::test]
async fn active_cancellation_and_shutdown_have_terminal_audit_records() -> Result<()> {
    let backend = Backend::start(SUCCESS).await?;
    let mut daemon = Daemon::start(&backend.endpoint).await?;
    let session = daemon.create().await?;
    let running = infer(daemon.path().into(), session.id);
    backend.wait_requests(1).await?;
    call(daemon.path(), Operation::Cancel { id: session.id }).await?;
    assert!(running.await?.is_err());
    assert_eq!(daemon.state(session.id).await?, SessionState::Cancelled);
    let session = daemon.create().await?;
    let running = infer(daemon.path().into(), session.id);
    backend.wait_requests(2).await?;
    daemon.finish().await?;
    assert!(running.await?.is_err());
    let snapshot: Value =
        serde_json::from_slice(&std::fs::read(daemon.path().join("sessions.json"))?)?;
    assert_eq!(
        snapshot["sessions"][session.id.to_string()]["state"],
        "failed"
    );
    Ok(())
}

#[tokio::test]
async fn invalid_backend_response_marks_failure() -> Result<()> {
    let backend = Backend::start(r#"{"error":"private server detail"}"#).await?;
    backend.release.add_permits(1);
    let mut daemon = Daemon::start(&backend.endpoint).await?;
    let session = daemon.create().await?;
    let error = infer(daemon.path().into(), session.id).await?.unwrap_err();
    assert!(!error.to_string().contains("private server detail"));
    assert_eq!(daemon.state(session.id).await?, SessionState::Failed);
    daemon.finish().await?;
    Ok(())
}

async fn raw(path: &Path, bytes: &[u8]) -> Result<Response> {
    let mut socket = UnixStream::connect(path.join("agentd.sock")).await?;
    socket.write_all(bytes).await?;
    let mut response = String::new();
    BufReader::new(socket).read_line(&mut response).await?;
    Ok(serde_json::from_str(&response)?)
}

#[tokio::test]
async fn protocol_rejects_oversize_unknown_fields_and_versions() -> Result<()> {
    let mut daemon = Daemon::start("http://127.0.0.1:1").await?;
    let mut bytes = serde_json::to_vec(&Request {
        version: API_VERSION + 1,
        operation: Operation::List,
    })?;
    bytes.push(b'\n');
    assert!(raw(daemon.path(), &bytes).await?.result.is_err());
    assert!(
        raw(
            daemon.path(),
            b"{\"version\":1,\"operation\":{\"type\":\"list\"},\"root\":true}\n"
        )
        .await?
        .result
        .is_err()
    );
    let oversized = vec![b'x'; MAX_FRAME_BYTES + 1];
    assert!(raw(daemon.path(), &oversized).await?.result.is_err());
    let session = daemon.create().await?;
    assert!(
        call(
            daemon.path(),
            Operation::Infer {
                id: session.id,
                prompt: " ".into()
            }
        )
        .await
        .is_err()
    );
    assert_eq!(daemon.state(session.id).await?, SessionState::Created);
    assert!(
        call(
            daemon.path(),
            Operation::Status {
                id: uuid::Uuid::new_v4()
            }
        )
        .await
        .is_err()
    );
    let adapters = call(daemon.path(), Operation::Adapters).await?;
    assert_eq!(
        adapters[0]["capabilities"],
        json!(["launch", "input", "cancellation"])
    );
    assert!(
        cerynth_adapter_generic_pty::GenericPtyAdapter
            .prepare_launch(
                &session,
                cerynth_agent_api::AdapterInput::Prompt("unsupported generic prompt".into())
            )
            .await
            .is_err()
    );
    daemon.finish().await?;
    Ok(())
}
