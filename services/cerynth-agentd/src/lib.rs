//! Unprivileged managed agent supervisor and session registry.

pub mod a2a_http;
mod effects;
mod gateway;
mod gpu;
mod guard;
mod harnesses;
mod inference;
mod model_residency;
mod native_guard;
mod orchestration;
mod process;
mod registry;
pub mod remote;
mod resources;
mod shared_resources;
mod supervisor;
mod workspace;

use std::{
    collections::HashMap,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{
    API_VERSION, HarnessKind, MAX_FRAME_BYTES, MAX_PROMPT_BYTES, MAX_RESPONSE_BYTES, Operation,
    Request, Response, SessionState,
};
use cerynth_model_api::OllamaClient;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{Mutex, Semaphore, watch},
    task::JoinSet,
};
use uuid::Uuid;

use registry::Registry;

fn validate_request(request: &Request) -> Result<()> {
    ensure!(
        request.version == API_VERSION,
        "unsupported agent API version"
    );
    if let Some(expected) = &request.expected_contract {
        ensure!(
            cerynth_agent_api::contract::manifest()["contract_id"] == *expected,
            "contract fingerprint mismatch; operation not dispatched"
        );
    }
    Ok(())
}

struct Runtime {
    registry: Registry,
    active: HashMap<Uuid, watch::Sender<bool>>,
    processes: HashMap<Uuid, supervisor::ProcessControl>,
}

pub struct AgentService {
    runtime: Mutex<Runtime>,
    backend: OllamaClient,
    /// Bounded cooperative request admission, not a GPU memory guarantee.
    inference_slots: Semaphore,
    directory: PathBuf,
    allow_unisolated: bool,
    workers: Mutex<JoinSet<()>>,
    gpu: Mutex<cerynth_resources::GpuBroker>,
    inference_control: Option<inference::InferenceControl>,
    native_opencode: Option<cerynth_agent_protocols::opencode::OpenCodeClient>,
    guard_control: Option<cerynth_agent_protocols::guard::GuardClient>,
    guard_cleanup_lock: Mutex<()>,
    native_guards: Mutex<std::collections::BTreeMap<Uuid, native_guard::OwnedGuard>>,
    resources: Option<cerynth_resources::systemd::Hierarchy>,
    resource_lock: Mutex<()>,
    shared_resources: Option<shared_resources::Client>,
    effects: Arc<effects::Journal>,
    effect_gateways: Mutex<std::collections::BTreeMap<Uuid, effects::Gateway>>,
}

impl AgentService {
    /// # Errors
    /// Returns an error if state cannot be locked or recovered, or the endpoint is invalid.
    pub fn open(directory: &Path, endpoint: &str) -> Result<Self> {
        Self::open_with_policy(directory, endpoint, false)
    }

    /// # Errors
    /// Returns an error for invalid state, lock contention, or unsupported endpoints.
    pub fn open_with_policy(
        directory: &Path,
        endpoint: &str,
        allow_unisolated: bool,
    ) -> Result<Self> {
        let (registry, (resources, shared_resources)) = Registry::open_checked(directory, || {
            let resources = resources::from_environment(directory)?;
            let shared = shared_resources::Client::open(directory, resources.is_some())?;
            Ok((resources, shared))
        })?;
        Ok(Self {
            runtime: Mutex::new(Runtime {
                registry,
                active: HashMap::new(),
                processes: HashMap::new(),
            }),
            backend: OllamaClient::new(endpoint)?,
            inference_slots: Semaphore::new(1),
            directory: directory.into(),
            allow_unisolated,
            workers: Mutex::new(JoinSet::new()),
            gpu: Mutex::new(cerynth_resources::GpuBroker::new(1024 * 1024 * 1024)),
            inference_control: inference::InferenceControl::from_environment()?,
            guard_control: std::env::var_os("CERYNTH_GUARD_SOCKET")
                .map(|socket| {
                    cerynth_agent_protocols::guard::GuardClient::new(PathBuf::from(socket))
                })
                .transpose()?,
            guard_cleanup_lock: Mutex::new(()),
            native_guards: Mutex::new(std::collections::BTreeMap::new()),
            resources,
            shared_resources,
            resource_lock: Mutex::new(()),
            effects: Arc::new(effects::Journal::open(&directory.join("tool-effects"))?),
            effect_gateways: Mutex::new(std::collections::BTreeMap::new()),
            native_opencode: std::env::var("CERYNTH_OPENCODE_URL")
                .ok()
                .map(|url| {
                    cerynth_agent_protocols::opencode::OpenCodeClient::new(
                        &url,
                        std::env::var("CERYNTH_OPENCODE_PASSWORD_ENV")
                            .ok()
                            .as_deref(),
                    )
                })
                .transpose()?,
        })
    }

    async fn dispatch(self: &Arc<Self>, request: Request) -> Result<Value> {
        validate_request(&request)?;
        match request.operation {
            operation @ (Operation::Protocol
            | Operation::PlanGlobalResources { .. }
            | Operation::ToolAwaitApproval { .. }
            | Operation::ToolDecide { .. }
            | Operation::ToolEffects { .. }
            | Operation::ProcessTree { .. }) => self.session_control_operation(operation).await,
            Operation::Create { spec } => Ok(serde_json::to_value(
                self.runtime.lock().await.registry.create(spec)?,
            )?),
            Operation::Retry {
                id,
                acknowledge_prior_effects,
            } => self.retry_session(id, acknowledge_prior_effects).await,
            Operation::List => self.session_records(None).await,
            Operation::Status { id } => self.session_records(Some(id)).await,
            Operation::Models => Ok(serde_json::to_value(self.backend.models().await?)?),
            Operation::Adapters => self.adapter_status().await,
            Operation::DiscoverHarness { harness } => self.discover_harness(harness).await,
            Operation::Cancel { id } | Operation::Stop { id } => self.stop_session(id).await,
            Operation::Infer { id, prompt } => self.infer(id, prompt).await,
            Operation::Start {
                id,
                operation_id,
                launch,
            } => self.start_generic(id, operation_id, launch).await,
            Operation::HarnessStart {
                id,
                operation_id,
                input,
            } => {
                let session = self.runtime.lock().await.registry.get(id)?;
                let launch = harnesses::launch(&session, input).await?;
                self.start_process(id, operation_id, launch).await
            }
            Operation::Input { id, input } => self.process_input(id, input).await,
            Operation::Output { id, offset, limit } => self.output(id, offset, limit).await,
            Operation::Events { id, after } => self.events(id, after).await,
            Operation::Delegate { parent, spec, task } => self.delegate(parent, spec, task).await,
            Operation::SendMessage {
                sender,
                recipient,
                content,
            } => self.message(sender, recipient, content).await,
            Operation::Messages { id } => Ok(serde_json::to_value(
                self.runtime
                    .lock()
                    .await
                    .registry
                    .get(id)?
                    .execution
                    .messages,
            )?),
            Operation::AuthorizeSharing { left, right } => {
                self.authorize_sharing(left, right).await
            }
            Operation::Graph { project } => self.graph(&project).await,
            Operation::SetPhase { id, phase } => self.set_phase(id, phase).await,
            Operation::Worktree { id } => self.worktree(id).await,
            Operation::CollectArtifact { id, path, kind } => {
                self.collect_artifact(id, path, kind).await
            }
            Operation::Artifacts { id } => Ok(serde_json::to_value(
                self.runtime
                    .lock()
                    .await
                    .registry
                    .get(id)?
                    .execution
                    .artifacts,
            )?),
            Operation::Diagnostics => self.diagnostics().await,
            Operation::ResourceAdmit { id, minimum } => {
                self.admit_resources(id, minimum.as_ref()).await
            }
            Operation::ResourceAccounting { id } => self.resource_accounting(id).await,
            Operation::PlatformResources { id } => self.platform_resources(id).await,
            Operation::ResourceView => self.resource_view().await,
            Operation::Observe { id, pid } => self.observe(id, pid).await,
            Operation::A2a { sponsor, request } => self.a2a(sponsor, request).await,
            Operation::Archive { id } => self.archive_session(id).await,
            Operation::Cleanup { id } => self.cleanup_session(id).await,
            operation @ (Operation::Inventory
            | Operation::GpuQuiesceModel { .. }
            | Operation::GpuRun { .. }
            | Operation::GpuSubmit { .. }
            | Operation::GpuJobs
            | Operation::GpuFinish { .. }
            | Operation::RequestResources { .. }) => self.resource_operation(operation).await,
        }
    }

    async fn session_control_operation(self: &Arc<Self>, operation: Operation) -> Result<Value> {
        match operation {
            Operation::Protocol => Ok(cerynth_agent_api::contract::manifest()),
            Operation::PlanGlobalResources {
                id,
                minimum,
                gpu_device_id,
            } => self.plan_global_resources(id, minimum, gpu_device_id).await,
            Operation::ToolAwaitApproval { id, approval_id } => {
                self.tool_await(id, approval_id).await
            }
            Operation::ToolDecide { id, request } => self.tool_decide(id, request).await,
            Operation::ToolEffects { id } => {
                self.runtime.lock().await.registry.get(id)?;
                Ok(self.effects.status(id).await)
            }
            Operation::ProcessTree { id } => {
                let s = self.runtime.lock().await.registry.get(id)?;
                Ok(serde_json::to_value(
                    s.execution
                        .process
                        .as_ref()
                        .map(process::descendants)
                        .unwrap_or_default(),
                )?)
            }
            _ => anyhow::bail!("unsupported session control operation"),
        }
    }

    async fn resource_operation(self: &Arc<Self>, operation: Operation) -> Result<Value> {
        match operation {
            Operation::GpuQuiesceModel {
                id,
                lease_id,
                endpoint_id,
                model,
            } => {
                self.quiesce_gpu_model(id, lease_id, endpoint_id, model)
                    .await
            }
            Operation::GpuRun {
                id,
                job_id,
                operation_id,
                launch,
            } => self.run_gpu(id, job_id, operation_id, launch).await,
            Operation::Inventory => {
                Ok(serde_json::to_value(cerynth_resources::inventory().await?)?)
            }
            Operation::GpuSubmit {
                id,
                device_id,
                memory_bytes,
                priority,
            } => {
                let session = self.runtime.lock().await.registry.get(id)?;
                ensure!(
                    !session.state.is_terminal()
                        && session.spec.runtime.capabilities.contains("gpu.execute"),
                    "GPU execution is not authorized for this session"
                );
                ensure!(
                    memory_bytes <= session.spec.runtime.budget.gpu_memory_bytes,
                    "GPU request exceeds the session reservation"
                );
                let inventory = cerynth_resources::inventory().await?;
                let sessions = self.runtime.lock().await.registry.list();
                let mut gpu = self.gpu.lock().await;
                gpu.update_devices(inventory.gpus);
                if self.shared_resources.is_some() {
                    self.check_shared_gpu(&sessions, &gpu, id, &device_id, memory_bytes)?;
                }
                gpu.set_pressure(
                    self.resources.is_some()
                        && cerynth_resources::pressure::sample()
                            .blocks_new_work(registry::now_ms()),
                );
                let committed = gpu
                    .jobs
                    .values()
                    .filter(|job| {
                        job.request.session_id == id
                            && matches!(
                                job.state,
                                cerynth_resources::JobState::Queued
                                    | cerynth_resources::JobState::Admitted
                            )
                    })
                    .fold(0_u64, |sum, job| {
                        sum.saturating_add(job.request.memory_bytes)
                    });
                ensure!(
                    committed.saturating_add(memory_bytes)
                        <= session.spec.runtime.budget.gpu_memory_bytes,
                    "aggregate GPU requests exceed the session budget"
                );
                Ok(serde_json::to_value(gpu.submit(
                    cerynth_resources::GpuRequest {
                        session_id: id,
                        device_id,
                        memory_bytes,
                        priority,
                        timeout_seconds: session.spec.runtime.budget.timeout_seconds,
                    },
                    registry::now_ms(),
                )?)?)
            }
            Operation::GpuJobs => self.gpu_jobs().await,
            Operation::GpuFinish {
                id,
                job_id,
                succeeded,
            } => self.finish_unowned_gpu(id, job_id, succeeded).await,
            Operation::RequestResources { id, requested } => {
                self.request_resources(id, requested).await
            }
            _ => anyhow::bail!("invalid resource operation"),
        }
    }

    async fn request_resources(
        &self,
        id: Uuid,
        requested: cerynth_agent_api::ResourceBudget,
    ) -> Result<Value> {
        requested.validate().map_err(anyhow::Error::msg)?;
        let mut runtime = self.runtime.lock().await;
        let current = runtime.registry.get(id)?;
        ensure!(!current.state.is_terminal(), "session is terminal");
        runtime.registry.update(id, |session| {
            registry::Registry::record_event(session, "resource_change_requested");
            Ok(())
        })?;
        Ok(
            serde_json::json!({"decision":if requested.fits_within(&current.spec.runtime.budget) {"within_existing_budget"}
            else {"queued_for_owner_review"},"applied":false,"requested":requested,
            "reason":"resource grants cannot be increased by agent requests; Person 1 admission integration is pending"}),
        )
    }

    async fn infer(&self, id: Uuid, prompt: String) -> Result<Value> {
        ensure!(
            !prompt.trim().is_empty() && prompt.len() <= MAX_PROMPT_BYTES,
            "prompt must contain 1..32768 bytes"
        );
        let (model, mut cancelled) = {
            let mut runtime = self.runtime.lock().await;
            let session = runtime.registry.get(id)?;
            ensure!(
                session.spec.harness == HarnessKind::InferenceProbe,
                "infer is only for inference_probe; use harness_start for agents"
            );
            ensure!(
                session.spec.model.mode == cerynth_agent_api::ModelMode::Local
                    && session.spec.model.endpoint_id.is_none(),
                "inference_probe uses only the daemon's local Ollama origin; named endpoints require a harness session"
            );
            ensure!(
                session.state == SessionState::Created,
                "session has already started"
            );
            ensure!(
                runtime.active.len() < 8,
                "inference queue is full (limit: 8 requests)"
            );
            runtime.registry.transition(
                id,
                SessionState::Preparing,
                "inference_probe_preparing",
            )?;
            runtime
                .registry
                .transition(id, SessionState::Starting, "inference_probe_starting")?;
            runtime
                .registry
                .transition(id, SessionState::Running, "inference_probe_started")?;
            runtime.registry.transition(
                id,
                SessionState::WaitingForInference,
                "inference_queued",
            )?;
            let (cancel, cancelled) = watch::channel(false);
            runtime.active.insert(id, cancel);
            (session.spec.model.name, cancelled)
        };
        let result = tokio::select! {
            biased;
            _ = cancelled.changed() => Err(anyhow::anyhow!("inference request cancelled")),
            result = tokio::time::timeout(Duration::from_secs(180), async {
                let _permit = self.inference_slots.acquire().await?;
                // Keep queued and executing requests distinguishable in the timeline.
                self.runtime.lock().await.registry.transition(id, SessionState::Running, "inference_started")?;
                self.backend.infer(&model, &prompt).await
            }) => result.context("inference queue deadline exceeded").and_then(|value| value),
        };
        let mut runtime = self.runtime.lock().await;
        runtime.active.remove(&id);
        if runtime.registry.get(id)?.state == SessionState::Cancelled {
            anyhow::bail!("inference request cancelled; backend work may continue");
        }
        match result {
            Ok(output) => {
                runtime
                    .registry
                    .transition(id, SessionState::Completing, "inference_finished")?;
                runtime.registry.transition(
                    id,
                    SessionState::Completed,
                    "inference_probe_completed",
                )?;
                Ok(serde_json::to_value(output)?)
            }
            Err(error) => {
                runtime
                    .registry
                    .transition(id, SessionState::Failed, "inference_failed")?;
                Err(error)
            }
        }
    }

    async fn shutdown(&self) -> Result<()> {
        let mut runtime = self.runtime.lock().await;
        for control in runtime.processes.values() {
            let _ = control.cancel.send(true);
        }
        for session in runtime.registry.list() {
            if !session.state.is_terminal()
                && (session.state != SessionState::Created
                    || session.execution.model_residency_cleanup_pending
                    || session
                        .execution
                        .resource_allocation
                        .as_ref()
                        .is_some_and(|a| a.active))
            {
                runtime
                    .registry
                    .transition(session.id, SessionState::Failed, "daemon_shutdown")?;
            }
        }
        for (_, cancel) in runtime.active.drain() {
            let _ = cancel.send(true);
        }
        drop(runtime);
        let mut workers = self.workers.lock().await;
        while workers.join_next().await.is_some() {}
        drop(workers);
        self.release_guard_sessions().await?;
        self.release_model_residency().await?;
        self.release_resources().await?;
        if let Some(hierarchy) = &self.resources {
            cerynth_resources::systemd::stop(&hierarchy.root).await?;
        }
        Ok(())
    }
}

async fn read_frame(reader: &mut (impl AsyncRead + Unpin), limit: usize) -> Result<Vec<u8>> {
    // Taking at most limit+1 bytes also bounds clients that never send a newline.
    let mut bytes = Vec::new();
    BufReader::new(reader)
        .take((limit + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .await?;
    ensure!(bytes.len() <= limit, "JSON frame exceeds limit");
    ensure!(bytes.pop() == Some(b'\n'), "incomplete JSON frame");
    Ok(bytes)
}

async fn connection(mut stream: UnixStream, service: Arc<AgentService>) -> Result<()> {
    ensure!(
        stream.peer_cred()?.uid() == nix::unistd::getuid().as_raw(),
        "peer UID mismatch"
    );
    let request = tokio::time::timeout(
        Duration::from_secs(5),
        read_frame(&mut stream, MAX_FRAME_BYTES),
    )
    .await?;
    let result = match request {
        Ok(bytes) => match serde_json::from_slice::<Request>(&bytes) {
            Ok(request) => Box::pin(service.dispatch(request))
                .await
                .map_err(|error| error.to_string()),
            Err(_) => Err("invalid agent API request".into()),
        },
        Err(error) => Err(error.to_string()),
    };
    let mut response = serde_json::to_vec(&Response {
        version: API_VERSION,
        result,
    })?;
    ensure!(
        response.len() < MAX_RESPONSE_BYTES,
        "response exceeds limit"
    );
    response.push(b'\n');
    tokio::time::timeout(Duration::from_secs(5), stream.write_all(&response)).await??;
    Ok(())
}

/// Run until SIGINT/SIGTERM or until `shutdown` resolves. Socket and state are user-private.
///
/// # Errors
/// Returns an error on bind, registry, or listener failure.
pub async fn serve(
    directory: PathBuf,
    endpoint: &str,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    serve_with_policy(directory, endpoint, false, shutdown).await
}

/// # Errors
/// Returns an error for unsafe directory/socket paths, state failures, or listener errors.
pub async fn serve_with_policy(
    directory: PathBuf,
    endpoint: &str,
    allow_unisolated: bool,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    let service = Arc::new(AgentService::open_with_policy(
        &directory,
        endpoint,
        allow_unisolated,
    )?);
    service.recover_inference_grants().await;
    service.recover_effect_sockets().await?;
    service.recover_native_guard_secrets().await?;
    service.release_guard_sessions().await?;
    // An unavailable coordinator retains the durable lease for an explicit
    // cleanup retry; it must not prevent inspection of recovered sessions.
    let _ = service.release_model_residency().await;
    service.reconcile_shared_resources().await?;
    let pressure_sampler = service.pressure_sampler();
    let socket = directory.join("agentd.sock");
    if let Ok(metadata) = std::fs::symlink_metadata(&socket) {
        ensure!(
            metadata.file_type().is_socket(),
            "refusing to replace a non-socket path"
        );
        std::fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    let mut clients = JoinSet::new();
    tokio::pin!(shutdown);
    let outcome = loop {
        tokio::select! {
            () = &mut shutdown => break Ok(()),
            accepted = listener.accept(), if clients.len() < 32 => {
                match accepted {
                    Ok((stream, _)) => { clients.spawn(connection(stream, service.clone())); }
                    Err(error) => break Err(anyhow::Error::from(error)),
                }
            }
            Some(_) = clients.join_next(), if !clients.is_empty() => {}
        }
    };
    let cleanup = service.shutdown().await;
    if let Some(sampler) = pressure_sampler {
        sampler.abort();
        let _ = sampler.await;
    }
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    drop(listener);
    std::fs::remove_file(socket)?;
    cleanup?;
    outcome
}

/// Send a single request to the daemon, with bounded framing and a total deadline.
///
/// # Errors
/// Returns an error on connection failure, protocol mismatch, or daemon rejection.
pub async fn call(directory: &Path, operation: Operation) -> Result<Value> {
    let expected_contract = if matches!(operation, Operation::Protocol) {
        None
    } else {
        std::env::var("CERYNTH_AGENT_CONTRACT").ok()
    };
    if let Some(expected) = &expected_contract {
        ensure!(
            expected.len() == 64 && expected.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid expected contract fingerprint"
        );
    }
    call_once(directory, operation, expected_contract).await
}

async fn call_once(
    directory: &Path,
    operation: Operation,
    expected_contract: Option<String>,
) -> Result<Value> {
    registry::private_directory(directory)?;
    tokio::time::timeout(Duration::from_secs(190), async {
        let mut stream = UnixStream::connect(directory.join("agentd.sock"))
            .await
            .context("cannot connect to cerynth-agentd; start serve first")?;
        ensure!(
            stream.peer_cred()?.uid() == nix::unistd::getuid().as_raw(),
            "server UID mismatch"
        );
        let mut bytes = serde_json::to_vec(&Request {
            version: API_VERSION,
            expected_contract,
            operation,
        })?;
        ensure!(bytes.len() < MAX_FRAME_BYTES, "request exceeds limit");
        bytes.push(b'\n');
        stream.write_all(&bytes).await?;
        let response: Response =
            serde_json::from_slice(&read_frame(&mut stream, MAX_RESPONSE_BYTES).await?)?;
        ensure!(
            response.version == API_VERSION,
            "unsupported agent API response version"
        );
        response.result.map_err(anyhow::Error::msg)
    })
    .await
    .context("agent daemon request timed out")?
}
