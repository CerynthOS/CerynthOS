//! Owner-selected common reservation domain, independent of agent-mounted IPC.
use crate::{AgentService, registry::Registry};
use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{ResourceBudget, Session, SessionState};
use cerynth_resources::{authority::Authority, global::Demand};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub(crate) struct Client {
    pub authority: Authority,
    pub owner: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    version: u32,
    directory: PathBuf,
    member_directory: PathBuf,
    policy_hash: String,
}

impl Client {
    pub(crate) fn open(directory: &Path, hierarchy: bool) -> Result<Option<Self>> {
        let selected = std::env::var_os("CERYNTH_RESOURCE_AUTHORITY_DIR").map(PathBuf::from);
        Self::selected(directory, hierarchy, selected)
    }
    fn selected(
        directory: &Path,
        hierarchy: bool,
        selected: Option<PathBuf>,
    ) -> Result<Option<Self>> {
        let path = directory.join("resource-authority.json");
        let saved: Option<Binding> = if path.try_exists()? {
            let m = std::fs::symlink_metadata(&path)?;
            ensure!(
                m.is_file()
                    && m.uid() == nix::unistd::getuid().as_raw()
                    && m.mode() & 0o777 == 0o600
                    && m.len() < 8192,
                "invalid resource authority binding"
            );
            Some(serde_json::from_slice(&std::fs::read(&path)?)?)
        } else {
            None
        };
        if selected.is_none() && saved.is_none() {
            return Ok(None);
        }
        ensure!(
            hierarchy,
            "shared resource authority requires CERYNTH_RESOURCE_HIERARCHY=1, including on restart"
        );
        let member_directory = directory.canonicalize()?;
        let saved_hash = saved.as_ref().map(|b| b.policy_hash.clone());
        let authority_directory = if let Some(saved) = saved {
            ensure!(
                saved.version == 1 && saved.member_directory == member_directory,
                "resource authority member identity changed"
            );
            if let Some(selected) = selected {
                ensure!(
                    selected.is_absolute() && selected.canonicalize()? == saved.directory,
                    "cannot silently switch authority domains"
                );
            }
            saved.directory
        } else {
            let selected = selected.context("resource authority selection missing")?;
            ensure!(
                selected.is_absolute(),
                "resource authority directory must be absolute"
            );
            selected.canonicalize()?
        };
        let authority = Authority::open(&authority_directory)?;
        ensure!(
            saved_hash
                .as_ref()
                .is_none_or(|hash| hash == authority.policy_fingerprint()),
            "saved resource authority policy changed; migration requires review"
        );
        let state = authority.view()?;
        let physical = cerynth_resources::total_memory().context("physical RAM unknown")?;
        let cpus = u64::try_from(std::thread::available_parallelism()?.get())?;
        ensure!(
            state.policy.capacity.memory_bytes <= physical.saturating_sub(512 * 1024 * 1024)
                && state.policy.capacity.cpu_quota_micros_per_second
                    <= cpus.saturating_sub(2).max(1) * 1_000_000,
            "authority capacity must preserve physical system headroom"
        );
        if !path.exists() {
            let binding = Binding {
                version: 1,
                directory: authority_directory,
                member_directory: member_directory.clone(),
                policy_hash: authority.policy_fingerprint().into(),
            };
            let mut temp = tempfile::NamedTempFile::new_in(directory)?;
            temp.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))?;
            temp.write_all(&serde_json::to_vec(&binding)?)?;
            temp.as_file().sync_all()?;
            temp.persist(&path)?;
            std::fs::File::open(directory)?.sync_all()?;
        }
        Ok(Some(Self {
            authority,
            owner: format!(
                "{:x}",
                Sha256::digest(member_directory.as_os_str().as_encoded_bytes())
            ),
        }))
    }
    pub(crate) fn verify(&self, session: &Session) -> Result<()> {
        let state = self.authority.view()?;
        let claim = state
            .claims
            .get(&session.id)
            .context("shared reservation missing; refusing launch")?;
        ensure!(
            claim.owner == self.owner && claim.granted == session.spec.runtime.budget,
            "shared allocation does not match session authority"
        );
        Ok(())
    }
}

impl AgentService {
    pub(crate) fn check_shared_gpu(
        &self,
        sessions: &[Session],
        gpu: &cerynth_resources::GpuBroker,
        id: uuid::Uuid,
        device: &str,
        memory: u64,
    ) -> Result<()> {
        let client = self
            .shared_resources
            .as_ref()
            .context("shared authority unavailable")?;
        let state = client.authority.view()?;
        let mut current = id;
        loop {
            let ancestor = sessions
                .iter()
                .find(|s| s.id == current)
                .context("GPU parent identity missing")?;
            let mut subtree = vec![current];
            loop {
                let before = subtree.len();
                for session in sessions {
                    if session
                        .spec
                        .runtime
                        .parent_id
                        .is_some_and(|p| subtree.contains(&p))
                        && !subtree.contains(&session.id)
                    {
                        subtree.push(session.id);
                    }
                }
                if before == subtree.len() {
                    break;
                }
            }
            let committed = gpu
                .jobs
                .values()
                .filter(|job| {
                    subtree.contains(&job.request.session_id)
                        && matches!(
                            job.state,
                            cerynth_resources::JobState::Queued
                                | cerynth_resources::JobState::Admitted
                        )
                })
                .try_fold(memory, |total, job| {
                    total.checked_add(job.request.memory_bytes)
                })
                .context("GPU subtree accounting overflow")?;
            ensure!(
                committed <= ancestor.spec.runtime.budget.gpu_memory_bytes,
                "GPU subtree requests exceed ancestor reservation"
            );
            if let Some(parent) = ancestor.spec.runtime.parent_id {
                current = parent;
            } else {
                let claim = state
                    .claims
                    .get(&current)
                    .context("shared GPU root reservation missing")?;
                ensure!(
                    claim.owner == client.owner
                        && claim.granted == ancestor.spec.runtime.budget
                        && claim.demand.gpu_device_id.as_deref() == Some(device),
                    "GPU request is outside its shared device reservation"
                );
                return Ok(());
            }
        }
    }
    pub(crate) async fn reserve_shared(
        &self,
        session: &Session,
        requested: &ResourceBudget,
        minimum: Option<&ResourceBudget>,
    ) -> Result<cerynth_resources::admission::Decision> {
        let Some(client) = &self.shared_resources else {
            return Ok(cerynth_resources::admission::Decision::Accept {
                granted: requested.clone(),
            });
        };
        let policy = client.authority.view()?.policy;
        let device = if policy.accelerators.len() == 1 {
            policy.accelerators.keys().next().cloned()
        } else {
            None
        };
        let mut free = BTreeMap::new();
        if requested.gpu_memory_bytes > 0 {
            for gpu in cerynth_resources::inventory().await?.gpus {
                if let (Some(total), Some(used)) = (gpu.total_bytes, gpu.used_bytes) {
                    if let Some(policy) = policy.accelerators.get(&gpu.id) {
                        ensure!(
                            policy.total_bytes <= total,
                            "authority GPU capacity exceeds discovered hardware"
                        );
                        free.insert(gpu.id, total.saturating_sub(used));
                    }
                }
            }
        }
        let mut demand = Demand::for_session(session, minimum.cloned(), device);
        demand.request_id = session.id;
        demand.requested = requested.clone();
        let receipt = client.authority.reserve(
            &client.owner,
            &demand,
            &cerynth_resources::pressure::sample(),
            &free,
        )?;
        Ok(receipt.decision)
    }
    pub(crate) async fn reconcile_shared_resources(&self) -> Result<()> {
        let Some(client) = &self.shared_resources else {
            return Ok(());
        };
        let claims = client.authority.view()?.claims;
        let mut runtime = self.runtime.lock().await;
        for claim in claims.values().filter(|c| c.owner == client.owner) {
            let session = runtime
                .registry
                .get(claim.demand.session_id)
                .context("shared reservation lacks registry evidence; reconcile manually")?;
            let intent_only = session.state == SessionState::Created
                && session.execution.resource_allocation.is_none()
                && session.execution.process.is_none()
                && session.execution.resource_slice.is_none();
            let drained = session.state.is_terminal()
                && !session.execution.cleanup_pending
                && !session.execution.gpu_cleanup_pending
                && !session.execution.model_residency_cleanup_pending
                && !session.execution.guard_cleanup_pending
                && !session.execution.workspace_preparing
                && !session.execution.resource_cleanup_pending
                && session
                    .execution
                    .resource_allocation
                    .as_ref()
                    .is_none_or(|a| !a.active);
            if intent_only || drained {
                // Do not let a lost revocation acknowledgement recycle shared
                // inference capacity while old credentials could still work.
                drop(runtime);
                self.revoke_shared_inference(session.id).await?;
                client.authority.release(&client.owner, session.id)?;
                runtime = self.runtime.lock().await;
                runtime.registry.update(session.id, |s| {
                    Registry::record_event(s, "shared_resources_reconciled");
                    Ok(())
                })?;
            }
        }
        Ok(())
    }
    pub(crate) async fn revoke_shared_inference(&self, root: uuid::Uuid) -> Result<()> {
        let sessions = self.runtime.lock().await.registry.list();
        let mut ids = vec![root];
        loop {
            let before = ids.len();
            for session in &sessions {
                if session
                    .spec
                    .runtime
                    .parent_id
                    .is_some_and(|p| ids.contains(&p))
                    && !ids.contains(&session.id)
                {
                    ids.push(session.id);
                }
            }
            if before == ids.len() {
                break;
            }
        }
        for session in sessions
            .iter()
            .filter(|s| ids.contains(&s.id) && s.spec.model.endpoint_id.is_some())
        {
            let control = self
                .inference_control
                .as_ref()
                .context("shared inference cleanup requires its configured coordinator")?;
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                cerynth_inferenced::call(
                    control.directory(),
                    true,
                    serde_json::json!({"type":"revoke","session_id":session.id}),
                ),
            )
            .await?;
            if let Err(error) = result {
                ensure!(
                    error.to_string() == "unknown session grant",
                    "shared inference revocation is unconfirmed; capacity retained"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cerynth_resources::{
        admission::Capacity,
        authority::Config,
        pressure::{self, Sample},
    };

    fn initialize(root: &Path) -> Result<PathBuf> {
        let directory = root.join("authority");
        Authority::initialize(
            &directory,
            Config {
                version: 1,
                capacity: Capacity {
                    memory_bytes: 4 * 1024 * 1024,
                    cpu_quota_micros_per_second: 40_000,
                },
                inference_limit: 2,
                accelerators: BTreeMap::new(),
            },
        )?;
        Ok(directory)
    }

    #[test]
    fn persisted_binding_cannot_silently_switch_policy_or_disable_kernel_hierarchy() -> Result<()> {
        let root = tempfile::tempdir()?;
        let authority = initialize(root.path())?;
        let member = root.path().join("member");
        crate::registry::private_directory(&member)?;
        let client =
            Client::selected(&member, true, Some(authority.clone()))?.context("fixture client")?;
        assert!(Client::selected(&member, false, None).is_err());
        assert!(Client::selected(&member, true, None)?.is_some());
        let other = root.path().join("other");
        crate::registry::private_directory(&other)?;
        assert!(Client::selected(&member, true, Some(other)).is_err());
        let path = authority.join("authority.json");
        let mut state: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        state["policy"]["inference_limit"] = serde_json::json!(3);
        std::fs::write(path, serde_json::to_vec(&state)?)?;
        assert!(client.authority.view().is_err());
        assert!(
            Client::selected(&member, true, None).is_err(),
            "restart accepted a changed policy fingerprint"
        );
        Ok(())
    }

    #[tokio::test]
    async fn intent_gap_recovers_but_uncertain_cleanup_retains_capacity() -> Result<()> {
        let root = tempfile::tempdir()?;
        let authority = initialize(root.path())?;
        let member = root.path().join("member");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace)?;
        let mut service = AgentService::open(&member, "http://127.0.0.1:9")?;
        service.shared_resources = Client::selected(&member, true, Some(authority))?;
        let session = service.runtime.lock().await.registry.create(serde_json::from_value(serde_json::json!({
            "name":"shared-intent-fixture","harness":"generic_pty","workspace":workspace,"model":{"name":"none"},
            "runtime":{"budget":{"cpu_quota_micros_per_second":10_000,"memory_max_bytes":1_048_576,"memory_high_bytes":524_288,"inference_concurrency":0}}
        }))?)?;
        let demand = Demand::for_session(&session, None, None);
        let sample = Sample {
            version: 1,
            observed_at_ms: pressure::now_ms(),
            available_memory_bytes: Some(1024_u64.pow(3)),
            memory_some_avg10: Some(0.0),
            memory_full_avg10: Some(0.0),
            cpu_some_avg10: Some(0.0),
            io_full_avg10: Some(0.0),
        };
        let client = service
            .shared_resources
            .as_ref()
            .context("fixture client")?;
        client
            .authority
            .reserve(&client.owner, &demand, &sample, &BTreeMap::new())?;
        // Crash gap: durable shared claim exists, local allocation was never committed.
        service.reconcile_shared_resources().await?;
        assert!(client.authority.view()?.claims.is_empty());
        client
            .authority
            .reserve(&client.owner, &demand, &sample, &BTreeMap::new())?;
        service.runtime.lock().await.registry.transition(
            session.id,
            SessionState::Cancelled,
            "fixture_stop",
        )?;
        service
            .runtime
            .lock()
            .await
            .registry
            .update(session.id, |s| {
                s.execution.cleanup_pending = true;
                Ok(())
            })?;
        service.reconcile_shared_resources().await?;
        assert_eq!(
            client.authority.view()?.claims.len(),
            1,
            "uncertain cleanup released live capacity"
        );
        service
            .runtime
            .lock()
            .await
            .registry
            .update(session.id, |s| {
                s.execution.cleanup_pending = false;
                Ok(())
            })?;
        service.reconcile_shared_resources().await?;
        assert!(client.authority.view()?.claims.is_empty());
        client
            .authority
            .reserve(&client.owner, &demand, &sample, &BTreeMap::new())?;
        service
            .runtime
            .lock()
            .await
            .registry
            .update(session.id, |s| {
                s.spec.model.endpoint_id = Some("unavailable-coordinator".into());
                Ok(())
            })?;
        assert!(service.reconcile_shared_resources().await.is_err());
        assert_eq!(
            client.authority.view()?.claims.len(),
            1,
            "unconfirmed inference revocation released capacity"
        );
        assert!(service.archive_session(session.id).await.is_err());
        assert!(
            service
                .runtime
                .lock()
                .await
                .registry
                .get(session.id)
                .is_ok(),
            "archive forgot an unconfirmed credential"
        );
        Ok(())
    }
}
