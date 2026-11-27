//! Shared inference coordinator with separate administrative and token-authenticated sockets.

mod backend;
mod backend_scope;
mod models;
mod privacy;
pub mod proxy;
mod residency;
use anyhow::{Context, Result, ensure};
pub use backend::OllamaLaunch;
use cerynth_model_api::{
    InferenceMetrics, OllamaClient,
    admission::AdmissionQueue,
    compatible::{CompatibleClient, CompatibleEndpoint},
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{Mutex, RwLock, watch},
    task::JoinSet,
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EndpointSpec {
    Ollama {
        id: String,
        url: String,
    },
    Compatible {
        id: String,
        config: CompatibleEndpoint,
    },
}

#[derive(Clone)]
enum Backend {
    Ollama(OllamaClient),
    Compatible(CompatibleClient),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionGrant {
    pub session_id: Uuid,
    pub endpoint_id: String,
    pub trust_domain: String,
    pub allowed_models: BTreeSet<String>,
    /// Include the session ID, project ID, and all parent session IDs as distinct scopes.
    pub concurrency_limits: Vec<(String, usize)>,
    pub max_priority: i32,
    #[serde(default)]
    pub allow_cloud: bool,
    #[serde(default)]
    pub disable_thinking: bool,
}

struct Grant {
    spec: SessionGrant,
    token_hash: [u8; 32],
    revoked: watch::Sender<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSample {
    pub session_id: Uuid,
    pub endpoint_id: String,
    pub model: String,
    pub trace_id: Uuid,
    pub metrics: InferenceMetrics,
}

#[derive(Default)]
struct State {
    pressure: Option<cerynth_resources::pressure::Sample>,
    endpoints: BTreeMap<String, (EndpointSpec, Backend)>,
    grants: BTreeMap<Uuid, Grant>,
    metrics: Vec<MetricSample>,
    failures: u64,
    trust_domains: BTreeMap<String, String>,
    models: cerynth_model_api::registry::ModelRegistry,
}

pub struct Coordinator {
    privacy: privacy::Policy,
    residency: Mutex<residency::Residency>,
    state: Mutex<State>,
    queue: AdmissionQueue,
    lifecycle: Arc<RwLock<()>>,
    request_slots: Arc<tokio::sync::Semaphore>,
    backends: backend::BackendManager,
    storage: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRouting {
    version: u32,
    #[serde(default)]
    cache_privacy: Option<privacy::Policy>,
    endpoints: Vec<EndpointSpec>,
    trust_domains: BTreeMap<String, String>,
    #[serde(default)]
    models: cerynth_model_api::registry::ModelRegistry,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdminOperation {
    PlanCache {
        entries: Vec<cerynth_model_api::cache::CacheEntry>,
        capabilities: cerynth_model_api::cache::CacheCapabilities,
        trust_domain: String,
        needed_bytes: u64,
    },
    QuiesceModel {
        lease_id: Uuid,
        session_id: Uuid,
        endpoint_id: String,
        model: String,
    },
    ReleaseModel {
        lease_id: Uuid,
        session_id: Uuid,
    },
    Residency,
    Pressure {
        sample: cerynth_resources::pressure::Sample,
    },
    Configure {
        endpoint: EndpointSpec,
    },
    Register {
        grant: SessionGrant,
    },
    Revoke {
        session_id: Uuid,
    },
    Models {
        endpoint_id: String,
    },
    Details {
        endpoint_id: String,
        model: String,
    },
    Unload {
        endpoint_id: String,
        model: String,
    },
    StartBackend {
        spec: OllamaLaunch,
    },
    StopBackend {
        endpoint_id: String,
    },
    Health,
    Metrics,
    Registry {
        endpoint_id: String,
        model: Option<String>,
    },
    CachedModels,
    PlanPlacement {
        request: cerynth_model_api::placement::PlacementRequest,
        targets: Vec<cerynth_model_api::placement::PlacementTarget>,
        observations: Vec<cerynth_model_api::placement::SwitchObservation>,
    },
    Discover {
        candidates: Vec<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceRequest {
    pub session_id: Uuid,
    pub token: String,
    pub model: String,
    pub prompt: String,
    pub max_output_tokens: u32,
    pub priority: i32,
}

impl Coordinator {
    #[must_use]
    pub fn new(concurrency: usize) -> Self {
        Self {
            privacy: privacy::Policy::TrustDomain,
            residency: Mutex::new(residency::Residency::default()),
            state: Mutex::new(State::default()),
            queue: AdmissionQueue::new(concurrency),
            lifecycle: Arc::new(RwLock::new(())),
            request_slots: Arc::new(tokio::sync::Semaphore::new(64)),
            backends: backend::BackendManager::default(),
            storage: None,
        }
    }

    fn persistent(directory: &Path, concurrency: usize) -> Result<Self> {
        let mut coordinator = Self::new(concurrency);
        coordinator.privacy = privacy::Policy::from_environment()?;
        let path = directory.join("routing.json");
        if path.exists() {
            let metadata = std::fs::symlink_metadata(&path)?;
            ensure!(
                metadata.is_file()
                    && metadata.uid() == nix::unistd::getuid().as_raw()
                    && metadata.mode() & 0o777 == 0o600
                    && metadata.len() < 1024 * 1024,
                "routing state must be a private regular file smaller than 1 MiB"
            );
            let stored: StoredRouting =
                serde_json::from_slice(&std::fs::read(&path)?).context("invalid routing state")?;
            ensure!(
                matches!(stored.version, 1 | 2)
                    && stored.endpoints.len() <= 32
                    && stored.trust_domains.len() <= 1024,
                "unsupported routing state"
            );
            if stored.cache_privacy == Some(privacy::Policy::ManagedProcess) {
                ensure!(
                    std::env::var("CERYNTH_CACHE_PRIVACY").is_err()
                        || coordinator.privacy == privacy::Policy::ManagedProcess,
                    "persisted strict cache policy cannot be downgraded in place"
                );
                coordinator.privacy = privacy::Policy::ManagedProcess;
            }
            let state = coordinator.state.get_mut();
            state.trust_domains = stored.trust_domains;
            state.models = stored.models;
            ensure!(
                state.models.records.len() <= 256,
                "model registry exceeds limit"
            );
            state.models.invalidate_residency();
            for spec in stored.endpoints {
                let (id, backend) = match &spec {
                    EndpointSpec::Ollama { id, url } => {
                        (id.clone(), Backend::Ollama(OllamaClient::new(url)?))
                    }
                    EndpointSpec::Compatible { id, config } => (
                        id.clone(),
                        Backend::Compatible(CompatibleClient::new(config)?),
                    ),
                };
                state.endpoints.insert(id, (spec, backend));
            }
        }
        coordinator.storage = Some(path);
        coordinator.backends = backend::BackendManager::persistent(directory)?;
        if coordinator.privacy == privacy::Policy::ManagedProcess {
            // Old owned scopes are now empty, no grants survive, and every new
            // grant must bind a newly owned process with a fresh private home.
            coordinator.state.get_mut().trust_domains.clear();
        }
        *coordinator.residency.get_mut() = residency::Residency::load(directory)?;
        if coordinator.privacy == privacy::Policy::ManagedProcess {
            let state = coordinator
                .state
                .try_lock()
                .context("coordinator initialization lock unavailable")?;
            coordinator.persist(&state)?;
        }
        Ok(coordinator)
    }

    fn persist(&self, state: &State) -> Result<()> {
        use std::io::Write;
        if let Some(path) = &self.storage {
            let directory = path.parent().context("missing routing directory")?;
            let stored = StoredRouting {
                version: 2,
                cache_privacy: Some(self.privacy),
                endpoints: state
                    .endpoints
                    .values()
                    .map(|(spec, _)| spec.clone())
                    .collect(),
                trust_domains: state.trust_domains.clone(),
                models: state.models.clone(),
            };
            let bytes = serde_json::to_vec(&stored)?;
            ensure!(bytes.len() < 1024 * 1024, "routing state exceeds limit");
            let mut file = tempfile::NamedTempFile::new_in(directory)?;
            file.write_all(&bytes)?;
            file.as_file().sync_all()?;
            file.persist(path)?;
            std::fs::File::open(directory)?.sync_all()?;
        }
        Ok(())
    }

    /// Administrative operations must never be exposed through an agent-mounted socket.
    ///
    /// # Errors
    /// Returns an error on policy conflicts, unavailable backends, or invalid grants.
    pub async fn admin(&self, operation: AdminOperation) -> Result<Value> {
        match operation {
            AdminOperation::PlanCache {
                entries,
                capabilities,
                trust_domain,
                needed_bytes,
            } => Self::cache_plan(&entries, &capabilities, &trust_domain, needed_bytes),
            AdminOperation::QuiesceModel {
                lease_id,
                session_id,
                endpoint_id,
                model,
            } => {
                self.quiesce_model(lease_id, session_id, endpoint_id, model)
                    .await
            }
            AdminOperation::ReleaseModel {
                lease_id,
                session_id,
            } => self.release_model(lease_id, session_id).await,
            AdminOperation::Residency => self.residency_status().await,
            AdminOperation::Pressure { sample } => self.pressure(sample).await,
            AdminOperation::Registry { endpoint_id, model } => {
                self.registry(endpoint_id, model).await
            }
            AdminOperation::CachedModels => Ok(
                json!({"models":self.state.lock().await.models,"source":"last_observation_not_live"}),
            ),
            AdminOperation::Discover { candidates } => Self::discover(candidates).await,
            AdminOperation::PlanPlacement {
                request,
                targets,
                observations,
            } => Self::placement(&request, &targets, &observations),
            AdminOperation::Configure { endpoint } => self.configure(endpoint).await,
            AdminOperation::Register { grant } => self.register(grant).await,
            AdminOperation::Revoke { session_id } => {
                let grant = self
                    .state
                    .lock()
                    .await
                    .grants
                    .remove(&session_id)
                    .context("unknown session grant")?;
                let _ = grant.revoked.send(true);
                Ok(json!({"revoked":session_id,"backend_preemption_guaranteed":false}))
            }
            AdminOperation::StartBackend { spec } => {
                self.backends.start(&spec).await?;
                let configured = self
                    .configure(EndpointSpec::Ollama {
                        id: spec.id.clone(),
                        url: spec.endpoint,
                    })
                    .await;
                if configured.is_err() {
                    let _ = self.backends.stop(&spec.id).await;
                }
                configured
            }
            AdminOperation::StopBackend { endpoint_id } => {
                self.stop_private_backend(&endpoint_id).await
            }
            AdminOperation::Health => {
                let backend_processes = self.backends.status().await;
                let residency_holds = self.residency.lock().await.active_count();
                let state = self.state.lock().await;
                let (active, queued) = self.queue.counts();
                Ok(
                    json!({"version":1,"service_ready":true,"backend_processes":backend_processes,"endpoints":state.endpoints.keys().collect::<Vec<_>>(),
                    "grants":state.grants.len(),"active_requests":active,"queued_requests":queued,
                    "grants_survive_restart":false,"admission_pressure":state.pressure,
                    "cache_privacy":self.privacy,
                    "admitted_and_waiting_requests":64-self.request_slots.available_permits(),"request_limit":64,
                    "model_residency_holds":residency_holds,
                    "pressure_admission_blocked":state.pressure.as_ref().is_some_and(|s|s.blocks_new_work(cerynth_resources::pressure::now_ms()))}),
                )
            }
            AdminOperation::Metrics => self.metrics().await,
            AdminOperation::Models { ref endpoint_id }
            | AdminOperation::Details {
                ref endpoint_id,
                model: _,
            }
            | AdminOperation::Unload {
                ref endpoint_id,
                model: _,
            } => {
                // Handled below using a dedicated helper to retain the operation payload.
                self.backend_metadata(endpoint_id.clone(), operation).await
            }
        }
    }

    async fn metrics(&self) -> Result<Value> {
        let state = self.state.lock().await;
        let mut latency: Vec<_> = state
            .metrics
            .iter()
            .map(|sample| sample.metrics.queue_ms + sample.metrics.request_ms)
            .collect();
        latency.sort_by(f64::total_cmp);
        let percentile = |p: usize| {
            if latency.is_empty() {
                None
            } else {
                Some(latency[((latency.len() * p).div_ceil(100)).saturating_sub(1)])
            }
        };
        Ok(
            json!({"samples":state.metrics,"failures":state.failures,"p50_ms":percentile(50),"p95_ms":percentile(95)}),
        )
    }

    async fn configure(&self, endpoint: EndpointSpec) -> Result<Value> {
        self.require_private_backend(&endpoint).await?;
        let _exclusive = self
            .lifecycle
            .try_write()
            .context("inference or model actuation is active; endpoint replacement is deferred")?;
        let (id, backend) = match &endpoint {
            EndpointSpec::Ollama { id, url } => {
                (id.clone(), Backend::Ollama(OllamaClient::new(url)?))
            }
            EndpointSpec::Compatible { id, config } => (
                id.clone(),
                Backend::Compatible(CompatibleClient::new(config)?),
            ),
        };
        ensure!(!id.is_empty() && id.len() <= 128, "invalid endpoint ID");
        self.ensure_endpoint_not_leased(&id).await?;
        let mut state = self.state.lock().await;
        ensure!(
            !state.grants.values().any(|g| g.spec.endpoint_id == id),
            "cannot replace endpoint with active grants"
        );
        ensure!(
            state.endpoints.len() < 32 || state.endpoints.contains_key(&id),
            "endpoint registry full"
        );
        let previous_models = state.models.clone();
        state
            .models
            .records
            .retain(|_, record| record.endpoint_id != id);
        let previous = state.endpoints.insert(id.clone(), (endpoint, backend));
        if let Err(error) = self.persist(&state) {
            state.models = previous_models;
            state.endpoints.remove(&id);
            if let Some(previous) = previous {
                state.endpoints.insert(id, previous);
            }
            return Err(error);
        }
        Ok(json!({"endpoint_id":id}))
    }

    async fn register(&self, grant: SessionGrant) -> Result<Value> {
        ensure!(
            !grant.allowed_models.is_empty()
                && grant.allowed_models.len() <= 32
                && grant
                    .allowed_models
                    .iter()
                    .all(|m| !m.is_empty() && m.len() <= 256)
                && !grant.trust_domain.is_empty()
                && grant.trust_domain.len() <= 128
                && !grant.concurrency_limits.is_empty()
                && grant.concurrency_limits.len() <= 32
                && grant
                    .concurrency_limits
                    .iter()
                    .all(|(id, limit)| !id.is_empty()
                        && id.len() <= 256
                        && *limit > 0
                        && *limit <= 32)
                && (-100..=100).contains(&grant.max_priority),
            "invalid session grant"
        );
        let mut state = self.state.lock().await;
        ensure!(
            grant
                .concurrency_limits
                .iter()
                .all(|(key, limit)| state.grants.values().all(|other| other
                    .spec
                    .concurrency_limits
                    .iter()
                    .all(|(other_key, other_limit)| key != other_key || limit == other_limit))),
            "concurrency scope conflicts with an existing grant"
        );
        ensure!(
            state.endpoints.contains_key(&grant.endpoint_id),
            "unknown endpoint"
        );
        self.require_private_backend(&state.endpoints[&grant.endpoint_id].0)
            .await?;
        let route = match &state.endpoints[&grant.endpoint_id].0 {
            EndpointSpec::Ollama { url, .. } => cerynth_model_api::origin_key(url)?,
            EndpointSpec::Compatible { config, .. } => {
                ensure!(
                    grant.allow_cloud
                        || cerynth_model_api::origin_key(&config.url)?.starts_with("http://"),
                    "session has not consented to remote inference"
                );
                cerynth_model_api::origin_key(&config.url)?
            }
        };
        let cache_domain = self.privacy.domain(&grant);
        ensure!(
            state
                .trust_domains
                .get(&route)
                .is_none_or(|domain| *domain == cache_domain),
            "backend origin already belongs to another trust domain; use an isolated backend"
        );
        ensure!(
            state.grants.len() < 128 && !state.grants.contains_key(&grant.session_id),
            "grant exists or registry full"
        );
        // Neither Ollama nor a generic compatible API proves cache-tenant isolation.
        // Separate trust domains therefore require distinct configured endpoints.
        ensure!(
            !state
                .grants
                .values()
                .any(|other| other.spec.endpoint_id == grant.endpoint_id
                    && other.spec.trust_domain != grant.trust_domain),
            "backend cache isolation unavailable across trust domains"
        );
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let (revoked, _) = watch::channel(false);
        let session_id = grant.session_id;
        state.trust_domains.insert(route.clone(), cache_domain);
        ensure!(
            state.trust_domains.len() <= 1024,
            "trust-domain registry full"
        );
        // Persist the binding before issuing a token. Revocation/restart must not
        // make an old engine cache eligible for another domain.
        self.persist(&state)?;
        state.grants.insert(
            session_id,
            Grant {
                spec: grant,
                token_hash: hash,
                revoked,
            },
        );
        Ok(json!({"session_id":session_id,"token":token,"cache_privacy":self.privacy}))
    }

    async fn backend_metadata(
        &self,
        endpoint_id: String,
        operation: AdminOperation,
    ) -> Result<Value> {
        let backend = self
            .state
            .lock()
            .await
            .endpoints
            .get(&endpoint_id)
            .context("unknown endpoint")?
            .1
            .clone();
        let Backend::Ollama(backend) = backend else {
            anyhow::bail!("metadata operation unsupported by compatible backend");
        };
        match operation {
            AdminOperation::Models { .. } => Ok(
                json!({"installed":backend.models().await?,"running":backend.running_models().await?}),
            ),
            AdminOperation::Details { model, .. } => backend.model_details(&model).await,
            AdminOperation::Unload { model, .. } => {
                let _exclusive = self
                    .lifecycle
                    .try_write()
                    .context("inference is active or queued")?;
                ensure!(
                    self.queue.counts() == (0, 0),
                    "cannot unload while inference is active or queued"
                );
                backend.unload(&model).await
            }
            _ => anyhow::bail!("invalid metadata operation"),
        }
    }

    /// Authenticate a session-specific request; model/endpoint/hierarchy come from its grant.
    ///
    /// # Errors
    /// Returns an error on invalid credentials, unauthorized models, queue pressure, or backend failure.
    pub async fn infer(&self, request: InferenceRequest) -> Result<Value> {
        ensure!(
            !request.prompt.trim().is_empty()
                && request.prompt.len() <= 32768
                && request.max_output_tokens > 0
                && request.max_output_tokens <= 4096,
            "invalid inference input"
        );
        let (grant, backend, mut revoked) = {
            let state = self.state.lock().await;
            let grant = state
                .grants
                .get(&request.session_id)
                .context("unknown session grant")?;
            let hash: [u8; 32] = Sha256::digest(request.token.as_bytes()).into();
            let difference = hash
                .iter()
                .zip(grant.token_hash.iter())
                .fold(0_u8, |acc, (a, b)| acc | (a ^ b));
            ensure!(difference == 0, "invalid session credential");
            ensure!(
                grant.spec.allowed_models.contains(&request.model)
                    && request.priority <= grant.spec.max_priority
                    && request.priority >= -100,
                "model or priority is not authorized"
            );
            let backend = match state.endpoints[&grant.spec.endpoint_id].1.clone() {
                Backend::Compatible(client) => Backend::Compatible(
                    client.for_cache_scope(format!("{:x}", Sha256::digest(grant.token_hash))),
                ),
                backend @ Backend::Ollama(_) => backend,
            };
            (grant.spec.clone(), backend, grant.revoked.subscribe())
        };
        let work = async {
            let _request_slot = self
                .request_slots
                .clone()
                .try_acquire_owned()
                .context("inference request admission is full")?;
            let waiting = Instant::now();
            let _lifecycle = self.lifecycle.read().await;
            let _residency = self.residency_permit(&grant.endpoint_id).await?;
            let _permit = self
                .queue
                .acquire(request.priority, grant.concurrency_limits)
                .await
                .map_err(anyhow::Error::msg)?;
            let start = Instant::now();
            let queue_ms = waiting.elapsed().as_secs_f64() * 1000.0;
            let (output, mut metrics) = match backend {
                Backend::Ollama(client) => {
                    client
                        .infer_streaming(&request.model, &request.prompt, request.max_output_tokens)
                        .await?
                }
                Backend::Compatible(client) => {
                    client
                        .infer(&request.model, &request.prompt, request.max_output_tokens)
                        .await?
                }
            };
            metrics.queue_ms = queue_ms;
            metrics.request_ms = start.elapsed().as_secs_f64() * 1000.0;
            let mut state = self.state.lock().await;
            if state.metrics.len() == 1024 {
                state.metrics.remove(0);
            }
            state.metrics.push(MetricSample {
                session_id: request.session_id,
                endpoint_id: grant.endpoint_id.clone(),
                model: request.model.clone(),
                trace_id: Uuid::new_v4(),
                metrics: metrics.clone(),
            });
            Ok::<_, anyhow::Error>(
                json!({"session_id":request.session_id,"endpoint_id":grant.endpoint_id,
                "response":output,"metrics":metrics}),
            )
        };
        let result = tokio::select! {
            biased;
            _ = revoked.changed() => Err(anyhow::anyhow!("session grant revoked; backend work may continue")),
            result = tokio::time::timeout(Duration::from_secs(180), work) => result.context("inference deadline exceeded").and_then(|r|r),
        };
        if result.is_err() {
            self.state.lock().await.failures += 1;
        }
        result
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    operation: Value,
}

#[derive(Deserialize)]
struct Response {
    version: u32,
    result: Result<Value, String>,
}

async fn frame(stream: &mut UnixStream, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    BufReader::new(stream)
        .take((limit + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .await?;
    ensure!(
        bytes.len() <= limit && bytes.pop() == Some(b'\n'),
        "invalid or oversized frame"
    );
    Ok(bytes)
}

async fn connection(
    mut stream: UnixStream,
    coordinator: Arc<Coordinator>,
    admin: bool,
) -> Result<()> {
    ensure!(
        stream.peer_cred()?.uid() == nix::unistd::getuid().as_raw(),
        "peer UID mismatch"
    );
    let bytes = tokio::time::timeout(Duration::from_secs(5), frame(&mut stream, 65536)).await??;
    let result = async {
        let request: Envelope =
            serde_json::from_slice(&bytes).context("invalid inference envelope")?;
        ensure!(
            request.version == 1,
            "unsupported inference protocol version"
        );
        if admin {
            coordinator
                .admin(
                    serde_json::from_value(request.operation)
                        .context("invalid administrative request")?,
                )
                .await
        } else {
            coordinator
                .infer(
                    serde_json::from_value(request.operation)
                        .context("invalid inference request")?,
                )
                .await
        }
    }
    .await
    .map_err(|error| error.to_string());
    let mut response = serde_json::to_vec(&json!({"version":1,"result":result}))?;
    ensure!(response.len() <= 4 * 1024 * 1024, "response exceeds limit");
    response.push(b'\n');
    tokio::time::timeout(Duration::from_secs(5), stream.write_all(&response)).await??;
    Ok(())
}

/// Run two private sockets; only inference.sock may be mounted into an agent sandbox.
///
/// # Errors
/// Returns an error for unsafe state permissions, lock contention, or listener failure.
pub async fn serve(
    directory: PathBuf,
    concurrency: usize,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    serve_with_http(directory, concurrency, None, shutdown).await
}

/// Run owner/data sockets and an optional authenticated loopback HTTP data plane.
///
/// # Errors
/// Returns an error for invalid bindings, unsafe state, or failed listeners.
pub async fn serve_with_http(
    directory: PathBuf,
    concurrency: usize,
    listen: Option<std::net::SocketAddr>,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    ensure!(
        listen.is_none_or(|address| address.ip().is_loopback()),
        "HTTP data plane must bind loopback"
    );
    if !directory.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)?;
    }
    let metadata = std::fs::symlink_metadata(&directory)?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == nix::unistd::getuid().as_raw()
            && metadata.mode() & 0o777 == 0o700,
        "inference directory must be user-owned mode 0700"
    );
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(directory.join("inferenced.lock"))?;
    lock.try_lock_exclusive()
        .context("inference daemon already running")?;
    let bind = |name: &str| -> Result<UnixListener> {
        let path = directory.join(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            use std::os::unix::fs::FileTypeExt;
            ensure!(
                metadata.file_type().is_socket(),
                "refusing to replace non-socket file"
            );
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    };
    let admin = bind("control.sock")?;
    let inference = bind("inference.sock")?;
    let coordinator = Arc::new(Coordinator::persistent(&directory, concurrency)?);
    let (http_stop, http_stopped) = tokio::sync::oneshot::channel();
    let mut http = if let Some(address) = listen {
        let listener = tokio::net::TcpListener::bind(address).await?;
        let router = proxy::router(coordinator.clone());
        Some(tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = http_stopped.await;
                })
                .await
        }))
    } else {
        None
    };
    let mut tasks = JoinSet::new();
    tokio::pin!(shutdown);
    let result = loop {
        tokio::select! {
            () = &mut shutdown => break Ok(()),
            result = async {match http.as_mut() {Some(task)=>task.await, None=>std::future::pending().await}}, if http.is_some() => {
                http.take();
                break result.context("HTTP proxy task failed").and_then(|result|result.map_err(anyhow::Error::from));
            }
            accepted = admin.accept(), if tasks.len() < 64 => {
                let (stream, _) = accepted?; tasks.spawn(connection(stream, coordinator.clone(), true));
            }
            accepted = inference.accept(), if tasks.len() < 56 => {
                let (stream, _) = accepted?; tasks.spawn(connection(stream, coordinator.clone(), false));
            }
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    };
    for (_, grant) in std::mem::take(&mut coordinator.state.lock().await.grants) {
        let _ = grant.revoked.send(true);
    }
    let _ = http_stop.send(());
    if let Some(mut http) = http
        && tokio::time::timeout(Duration::from_secs(5), &mut http)
            .await
            .is_err()
    {
        http.abort();
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    drop(admin);
    drop(inference);
    std::fs::remove_file(directory.join("control.sock"))?;
    std::fs::remove_file(directory.join("inference.sock"))?;
    result
}

/// # Errors
/// Returns an error for transport failure, unsupported protocol, or rejected requests.
pub async fn call(directory: &Path, admin: bool, operation: Value) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(190), async {
        let mut stream = UnixStream::connect(directory.join(if admin {
            "control.sock"
        } else {
            "inference.sock"
        }))
        .await?;
        ensure!(
            stream.peer_cred()?.uid() == nix::unistd::getuid().as_raw(),
            "server UID mismatch"
        );
        let mut request = serde_json::to_vec(&json!({"version":1,"operation":operation}))?;
        ensure!(request.len() < 65536, "request exceeds limit");
        request.push(b'\n');
        stream.write_all(&request).await?;
        let response: Response =
            serde_json::from_slice(&frame(&mut stream, 4 * 1024 * 1024).await?)?;
        ensure!(response.version == 1, "unsupported response version");
        response.result.map_err(anyhow::Error::msg)
    })
    .await
    .context("inference daemon deadline exceeded")?
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    fn grant(session_id: Uuid, domain: &str) -> SessionGrant {
        SessionGrant {
            session_id,
            endpoint_id: "local".into(),
            trust_domain: domain.into(),
            allowed_models: BTreeSet::from(["fixture".into()]),
            concurrency_limits: vec![("project".into(), 1)],
            max_priority: 0,
            allow_cloud: false,
            disable_thinking: false,
        }
    }

    #[tokio::test]
    async fn trust_bindings_survive_restart_but_session_credentials_do_not() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let coordinator = Coordinator::persistent(directory.path(), 1)?;
        coordinator
            .admin(AdminOperation::Configure {
                endpoint: EndpointSpec::Ollama {
                    id: "local".into(),
                    url: "http://127.0.0.1:1".into(),
                },
            })
            .await?;
        let id = Uuid::new_v4();
        let registration = coordinator
            .admin(AdminOperation::Register {
                grant: grant(id, "tenant-a"),
            })
            .await?;
        let token = registration["token"].as_str().unwrap().to_owned();
        drop(coordinator);
        let stored = std::fs::read_to_string(directory.path().join("routing.json"))?;
        assert!(!stored.contains(&token));
        let recovered = Coordinator::persistent(directory.path(), 1)?;
        assert!(
            recovered
                .infer(InferenceRequest {
                    session_id: id,
                    token,
                    model: "fixture".into(),
                    prompt: "context".into(),
                    max_output_tokens: 1,
                    priority: 0
                })
                .await
                .is_err()
        );
        assert!(
            recovered
                .admin(AdminOperation::Register {
                    grant: grant(Uuid::new_v4(), "tenant-b")
                })
                .await
                .is_err()
        );
        recovered
            .admin(AdminOperation::Register {
                grant: grant(Uuid::new_v4(), "tenant-a"),
            })
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn cloud_requires_both_owner_endpoint_consent_and_session_consent() -> Result<()> {
        use cerynth_model_api::compatible::CompatibleEndpoint;
        let coordinator = Coordinator::new(1);
        let mut spec = CompatibleEndpoint {
            url: "https://example.invalid/v1".into(),
            cloud_consent: false,
            credential_env: None,
            cache_isolation: cerynth_model_api::compatible::CacheIsolation::SharedTrustDomain,
        };
        assert!(
            coordinator
                .admin(AdminOperation::Configure {
                    endpoint: EndpointSpec::Compatible {
                        id: "local".into(),
                        config: spec.clone()
                    }
                })
                .await
                .is_err()
        );
        spec.cloud_consent = true;
        coordinator
            .admin(AdminOperation::Configure {
                endpoint: EndpointSpec::Compatible {
                    id: "local".into(),
                    config: spec,
                },
            })
            .await?;
        let mut session = grant(Uuid::new_v4(), "owner");
        assert!(
            coordinator
                .admin(AdminOperation::Register {
                    grant: session.clone()
                })
                .await
                .is_err()
        );
        session.allow_cloud = true;
        coordinator
            .admin(AdminOperation::Register { grant: session })
            .await?;
        Ok(())
    }
}
