//! Durable cooperative GPU/model exclusion. A restart never silently reopens a lease.
use crate::{Backend, Coordinator, EndpointSpec};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    lease_id: Uuid,
    session_id: Uuid,
    endpoint_id: String,
    origin: String,
    model: String,
    prepared: bool,
    released: bool,
    was_loaded: bool,
}
struct Lease {
    _guard: OwnedRwLockWriteGuard<()>,
}
#[derive(Default)]
pub(crate) struct Residency {
    records: BTreeMap<Uuid, Record>,
    leases: BTreeMap<Uuid, Lease>,
    gates: BTreeMap<String, Arc<RwLock<()>>>,
    path: Option<PathBuf>,
}

impl Residency {
    pub(crate) fn active_count(&self) -> usize {
        self.leases.len()
    }
    pub(crate) fn load(directory: &Path) -> Result<Self> {
        let path = directory.join("residency.json");
        let mut state = Self {
            path: Some(path.clone()),
            ..Self::default()
        };
        if path.exists() {
            let metadata = std::fs::symlink_metadata(&path)?;
            let uid = std::fs::metadata("/proc/self")?.uid();
            ensure!(
                metadata.is_file()
                    && metadata.uid() == uid
                    && metadata.mode() & 0o777 == 0o600
                    && metadata.len() <= 4 * 1024 * 1024,
                "invalid residency journal permissions/type/size"
            );
            let records: Vec<Record> = serde_json::from_slice(&std::fs::read(&path)?)?;
            ensure!(records.len() <= 4096, "residency ledger full");
            for record in records {
                ensure!(
                    record.model.len() <= 256
                        && record.endpoint_id.len() <= 128
                        && record.origin.len() <= 512,
                    "invalid residency metadata"
                );
                if !record.released {
                    let gate = state
                        .gates
                        .entry(record.origin.clone())
                        .or_default()
                        .clone();
                    let guard = gate
                        .try_write_owned()
                        .context("duplicate persisted residency origin")?;
                    state
                        .leases
                        .insert(record.lease_id, Lease { _guard: guard });
                }
                ensure!(
                    state.records.insert(record.lease_id, record).is_none(),
                    "duplicate residency operation ID"
                );
            }
            ensure!(
                state.leases.len() <= 32,
                "active residency lease bound exceeded"
            );
        }
        Ok(state)
    }

    fn persist(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path.parent().context("residency directory missing")?;
        let bytes = serde_json::to_vec(&self.records.values().collect::<Vec<_>>())?;
        ensure!(
            bytes.len() <= 4 * 1024 * 1024,
            "residency journal size exceeded"
        );
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}

impl Coordinator {
    pub(crate) async fn ensure_endpoint_not_leased(&self, endpoint: &str) -> Result<()> {
        ensure!(
            !self
                .residency
                .lock()
                .await
                .records
                .values()
                .any(|r| !r.released && r.endpoint_id == endpoint),
            "endpoint has an active model residency lease"
        );
        Ok(())
    }

    async fn residency_origin(&self, endpoint: &str) -> Result<String> {
        let state = self.state.lock().await;
        let (spec, _) = state.endpoints.get(endpoint).context("unknown endpoint")?;
        cerynth_model_api::origin_key(match spec {
            EndpointSpec::Ollama { url, .. } => url,
            EndpointSpec::Compatible { config, .. } => &config.url,
        })
    }

    pub(crate) async fn residency_permit(
        &self,
        endpoint: &str,
    ) -> Result<OwnedRwLockReadGuard<()>> {
        let origin = self.residency_origin(endpoint).await?;
        let gate = {
            let mut state = self.residency.lock().await;
            ensure!(
                state.gates.len() < 1024 || state.gates.contains_key(&origin),
                "residency origin registry full"
            );
            state.gates.entry(origin).or_default().clone()
        };
        Ok(gate.read_owned().await)
    }

    // Scope ownership, durable intent, engine actuation and acknowledgement stay
    // together so a lost reply cannot turn an unknown unload into a fresh lease.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn quiesce_model(
        &self,
        lease_id: Uuid,
        session_id: Uuid,
        endpoint_id: String,
        model: String,
    ) -> Result<Value> {
        ensure!(
            !model.is_empty() && model.len() <= 256 && !model.chars().any(char::is_control),
            "invalid model identity"
        );
        let origin = self.residency_origin(&endpoint_id).await?;
        {
            let state = self.residency.lock().await;
            if let Some(record) = state.records.get(&lease_id) {
                ensure!(
                    record.session_id == session_id
                        && record.endpoint_id == endpoint_id
                        && record.origin == origin
                        && record.model == model,
                    "residency operation ID reused with different details"
                );
                return Ok(
                    json!({"lease_id":lease_id,"ready":record.prepared && !record.released,"released":record.released,"replayed":true}),
                );
            }
        }
        let _exclusive = self
            .lifecycle
            .try_write()
            .context("inference is active or queued; model release is deferred")?;
        let client = {
            let state = self.state.lock().await;
            let (_, backend) = state
                .endpoints
                .get(&endpoint_id)
                .context("endpoint removed")?;
            let Backend::Ollama(client) = backend else {
                anyhow::bail!("backend has no validated residency actuation");
            };
            client.clone()
        };
        let before =
            tokio::time::timeout(Duration::from_secs(5), client.running_models()).await??;
        let was_loaded = contains_model(&before, &model)?;
        {
            let mut state = self.residency.lock().await;
            ensure!(
                state.records.len() < 4096 && state.leases.len() < 32,
                "residency ledger full"
            );
            ensure!(
                !state
                    .records
                    .values()
                    .any(|r| !r.released && r.origin == origin),
                "model backend is already reserved for a GPU job"
            );
            let gate = state.gates.entry(origin.clone()).or_default().clone();
            let guard = gate
                .try_write_owned()
                .context("model has active inference")?;
            let record = Record {
                lease_id,
                session_id,
                endpoint_id: endpoint_id.clone(),
                origin: origin.clone(),
                model: model.clone(),
                prepared: false,
                released: false,
                was_loaded,
            };
            state.records.insert(lease_id, record);
            state.leases.insert(lease_id, Lease { _guard: guard });
            // If persistence fails, keep the in-memory gate closed. No engine
            // action has happened; the caller can inspect/release this lease.
            state.persist()?;
        }
        if was_loaded {
            tokio::time::timeout(Duration::from_secs(10), client.unload(&model)).await??;
        }
        let after = tokio::time::timeout(Duration::from_secs(5), client.running_models()).await??;
        ensure!(
            !contains_model(&after, &model)?,
            "backend did not confirm model unload"
        );
        let mut state = self.residency.lock().await;
        state
            .records
            .get_mut(&lease_id)
            .context("residency lease disappeared")?
            .prepared = true;
        state.persist()?;
        Ok(
            json!({"lease_id":lease_id,"ready":true,"was_loaded":was_loaded,"unloaded":was_loaded,
            "automatic_expiry":false,"resume":"lazy reload after verified GPU cleanup"}),
        )
    }

    pub(crate) async fn release_model(&self, lease_id: Uuid, session_id: Uuid) -> Result<Value> {
        {
            let state = self.residency.lock().await;
            let Some(record) = state.records.get(&lease_id) else {
                return Ok(json!({"released":true,"known":false}));
            };
            ensure!(
                record.session_id == session_id,
                "residency lease belongs to another session"
            );
            if record.released {
                return Ok(json!({"released":true,"replayed":true}));
            }
        }
        ensure_scope_empty(session_id).await?;
        let mut state = self.residency.lock().await;
        let record = state
            .records
            .get_mut(&lease_id)
            .context("residency lease disappeared")?;
        record.released = true;
        if let Err(error) = state.persist() {
            state
                .records
                .get_mut(&lease_id)
                .context("residency lease disappeared")?
                .released = false;
            return Err(error);
        }
        state.leases.remove(&lease_id);
        Ok(json!({"released":true,"lazy_model_reload_allowed":true}))
    }

    pub(crate) async fn residency_status(&self) -> Result<Value> {
        let state = self.residency.lock().await;
        Ok(
            json!({"leases":state.records.values().filter(|r|!r.released).collect::<Vec<_>>(),
            "retained_operations":state.records.len(),"automatic_expiry":false,"external_gpu_bypass_prevented":false}),
        )
    }
}

fn contains_model(response: &Value, model: &str) -> Result<bool> {
    let models = response["models"]
        .as_array()
        .context("invalid model residency response")?;
    ensure!(models.len() <= 256, "model observation bound exceeded");
    Ok(models
        .iter()
        .any(|r| r["name"] == model || r["model"] == model))
}

async fn ensure_scope_empty(session: Uuid) -> Result<()> {
    let unit = format!("cerynth-agent-{session}.scope");
    let uid = nix::unistd::getuid();
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new("/usr/bin/systemctl")
            .args([
                "--user",
                "show",
                &unit,
                "--property=ControlGroup",
                "--value",
            ])
            .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path=/run/user/{uid}/bus"),
            )
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    ensure!(output.status.success(), "cannot verify GPU scope cleanup");
    let path = String::from_utf8(output.stdout)?;
    let group = path.trim();
    if group.is_empty() {
        return Ok(());
    }
    ensure!(
        group.starts_with('/') && !group.split('/').any(|p| p == ".." || p == "."),
        "invalid kernel scope path"
    );
    let path = Path::new("/sys/fs/cgroup").join(group.trim_start_matches('/'));
    match std::fs::read_to_string(path.join("cgroup.events")) {
        Ok(events) => ensure!(
            events.lines().any(|line| line == "populated 0"),
            "GPU scope still contains processes"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AdminOperation;
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn residency_waiters_are_bounded_and_revocation_releases_admission_slots() -> Result<()> {
        use crate::{InferenceRequest, SessionGrant};
        let coordinator = Arc::new(Coordinator::new(1));
        let origin = "http://127.0.0.1:9";
        coordinator
            .admin(AdminOperation::Configure {
                endpoint: EndpointSpec::Ollama {
                    id: "local".into(),
                    url: origin.into(),
                },
            })
            .await?;
        let gate = coordinator
            .residency
            .lock()
            .await
            .gates
            .entry(origin.into())
            .or_default()
            .clone();
        let _hold = gate.write_owned().await;
        let id = Uuid::new_v4();
        let grant = coordinator
            .admin(AdminOperation::Register {
                grant: SessionGrant {
                    session_id: id,
                    endpoint_id: "local".into(),
                    trust_domain: "fixture".into(),
                    allowed_models: std::collections::BTreeSet::from(["fixture".into()]),
                    concurrency_limits: vec![(id.to_string(), 1)],
                    max_priority: 0,
                    allow_cloud: false,
                    disable_thinking: false,
                },
            })
            .await?;
        let request = || InferenceRequest {
            session_id: id,
            token: grant["token"].as_str().unwrap().into(),
            model: "fixture".into(),
            prompt: "public fixture".into(),
            max_output_tokens: 4,
            priority: 0,
        };
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..64 {
            let worker = coordinator.clone();
            let input = request();
            tasks.spawn(async move { worker.infer(input).await });
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while coordinator.request_slots.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert!(
            coordinator
                .infer(request())
                .await
                .unwrap_err()
                .to_string()
                .contains("admission is full")
        );
        coordinator
            .admin(AdminOperation::Revoke { session_id: id })
            .await?;
        while let Some(result) = tasks.join_next().await {
            assert!(result?.is_err());
        }
        assert_eq!(coordinator.request_slots.available_permits(), 64);
        Ok(())
    }

    #[tokio::test]
    async fn durable_unload_gate_survives_restart_and_rejects_substitution() -> Result<()> {
        let loaded = Arc::new(AtomicBool::new(true));
        let seen = loaded.clone();
        let unload = loaded.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let router=Router::new().route("/api/ps",get(move || {let loaded=seen.clone();async move {
            Json(json!({"models":if loaded.load(Ordering::SeqCst) {vec![json!({"name":"fixture:latest"})]} else {vec![]}}))
        }})).route("/api/generate",post(move |Json(request):Json<Value>| {let loaded=unload.clone();async move {
            assert_eq!(request["keep_alive"],0);loaded.store(false,Ordering::SeqCst);Json(json!({"done":true}))
        }}));
        let backend = tokio::spawn(async move { axum::serve(listener, router).await });
        let directory = tempfile::tempdir()?;
        let coordinator = Coordinator::persistent(directory.path(), 1)?;
        coordinator
            .admin(AdminOperation::Configure {
                endpoint: EndpointSpec::Ollama {
                    id: "local".into(),
                    url: origin.clone(),
                },
            })
            .await?;
        let session = Uuid::new_v4();
        let lease = Uuid::new_v4();
        assert_eq!(
            coordinator
                .quiesce_model(lease, session, "local".into(), "fixture:latest".into())
                .await?["ready"],
            true
        );
        assert!(!loaded.load(Ordering::SeqCst));
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                coordinator.residency_permit("local")
            )
            .await
            .is_err()
        );
        assert!(
            coordinator
                .quiesce_model(
                    lease,
                    Uuid::new_v4(),
                    "local".into(),
                    "fixture:latest".into()
                )
                .await
                .is_err()
        );
        assert!(
            coordinator
                .admin(AdminOperation::Configure {
                    endpoint: EndpointSpec::Ollama {
                        id: "local".into(),
                        url: origin
                    }
                })
                .await
                .is_err()
        );
        drop(coordinator);
        let recovered = Coordinator::persistent(directory.path(), 1)?;
        assert_eq!(
            recovered.residency_status().await?["leases"][0]["lease_id"],
            lease.to_string()
        );
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                recovered.residency_permit("local")
            )
            .await
            .is_err()
        );
        assert!(
            recovered
                .release_model(lease, Uuid::new_v4())
                .await
                .is_err()
        );
        // Scope verification is exercised by the real host fixture, not bypassed
        // with a test-only production API here.
        backend.abort();
        Ok(())
    }

    #[test]
    fn malformed_or_ambiguous_journals_do_not_reopen_inference() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("residency.json");
        std::fs::write(&path, b"not json")?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        assert!(Residency::load(directory.path()).is_err());
        let record = Record {
            lease_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            endpoint_id: "local".into(),
            origin: "http://127.0.0.1:11434".into(),
            model: "fixture".into(),
            prepared: false,
            released: false,
            was_loaded: false,
        };
        std::fs::write(&path, serde_json::to_vec(&vec![record.clone(), record])?)?;
        assert!(Residency::load(directory.path()).is_err());
        Ok(())
    }
}
