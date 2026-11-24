use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{
    API_VERSION, ExecutionBoundary, ExecutionRecord, Session, SessionEvent, SessionSpec,
    SessionState,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAX_SESSIONS: usize = 128;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    sessions: BTreeMap<Uuid, Session>,
    #[serde(default)]
    archived_operations: BTreeMap<Uuid, Uuid>,
}

pub struct Registry {
    path: PathBuf,
    snapshot: Snapshot,
    _lock: File,
}

pub fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

/// All persistent state and the socket live under one user-owned private directory.
pub fn private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == nix::unistd::getuid().as_raw()
            && metadata.mode() & 0o777 == 0o700,
        "runtime directory must be a user-owned directory with mode 0700"
    );
    Ok(())
}

impl Registry {
    pub(crate) fn phase_demands(&self) -> Vec<cerynth_resources::phase::Demand> {
        self.snapshot
            .sessions
            .values()
            .filter(|session| {
                session
                    .execution
                    .resource_allocation
                    .as_ref()
                    .is_some_and(|allocation| allocation.active && allocation.kernel_enforced)
            })
            .filter_map(|session| {
                session.execution.resource_slice.as_ref().map(|unit| {
                    cerynth_resources::phase::Demand {
                        id: session.id,
                        parent: session.spec.runtime.parent_id,
                        unit: unit.clone(),
                        phase: if session.state.is_terminal() {
                            cerynth_agent_api::WorkloadPhase::Cleanup
                        } else {
                            session.execution.phase
                        },
                        ceiling: session.spec.runtime.budget.cpu_weight,
                    }
                })
            })
            .collect()
    }
    #[cfg(test)]
    pub fn open(directory: &Path) -> Result<Self> {
        Self::open_checked(directory, || Ok(())).map(|(registry, ())| registry)
    }

    /// Validate member identity under the registry lock before recovering any
    /// processes. A copied registry must not stop the original member's scopes.
    pub fn open_checked<T>(
        directory: &Path,
        preflight: impl FnOnce() -> Result<T>,
    ) -> Result<(Self, T)> {
        private_directory(directory)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(directory.join("registry.lock"))?;
        lock.try_lock_exclusive()
            .context("another agent daemon owns this registry")?;
        let checked = preflight()?;
        let path = directory.join("sessions.json");
        let snapshot = if path.exists() {
            let metadata = fs::symlink_metadata(&path)?;
            ensure!(
                metadata.is_file()
                    && metadata.uid() == nix::unistd::getuid().as_raw()
                    && metadata.mode() & 0o777 == 0o600
                    && metadata.len() <= 4 * 1024 * 1024,
                "registry must be a private regular file smaller than 4 MiB"
            );
            let snapshot: Snapshot = serde_json::from_slice(&fs::read(&path)?)
                .context("invalid registry; refusing to discard session history")?;
            ensure!(
                snapshot.version == API_VERSION,
                "unsupported registry version"
            );
            ensure!(
                snapshot.sessions.len() <= MAX_SESSIONS,
                "registry exceeds session limit"
            );
            for (id, session) in &snapshot.sessions {
                ensure!(
                    *id == session.id && session.owner_uid == nix::unistd::getuid().as_raw(),
                    "registry identity mismatch"
                );
                session.spec.validate().map_err(anyhow::Error::msg)?;
            }
            snapshot
        } else {
            Snapshot {
                version: API_VERSION,
                sessions: BTreeMap::new(),
                archived_operations: BTreeMap::new(),
            }
        };
        let mut registry = Self {
            path,
            snapshot,
            _lock: lock,
        };
        let mut recovered = registry.snapshot.clone();
        for session in recovered.sessions.values_mut() {
            if session.execution.cleanup_pending
                || session.execution.model_residency_cleanup_pending
                || session.execution.resource_cleanup_pending
                || session
                    .execution
                    .resource_allocation
                    .as_ref()
                    .is_some_and(|a| a.active)
                || session.execution.gpu_cleanup_pending
                || session.execution.workspace_preparing
                || (session.execution.guard_cleanup_pending
                    && session.execution.native_guard_unit.is_some())
                || (session.state != SessionState::Created && !session.state.is_terminal())
            {
                crate::process::recover(session)?;
                session.execution.cleanup_pending = false;
                session.execution.resource_cleanup_pending = false;
                if let Some(allocation) = &mut session.execution.resource_allocation {
                    allocation.active = false;
                }
                session.execution.gpu_cleanup_pending = false;
                session.execution.workspace_preparing = false;
                if session.execution.native_guard_unit.is_some() {
                    session.execution.guard_cleanup_pending = false;
                }
                for operation in &mut session.execution.operations {
                    if operation.status == cerynth_agent_api::OperationStatus::Started {
                        operation.status = cerynth_agent_api::OperationStatus::Unknown;
                    }
                }
                session.execution.failure_reason = Some("daemon_restart".into());
                if !session.state.is_terminal() {
                    Self::change(session, SessionState::Failed, "daemon_restart")?;
                }
            }
        }
        registry.commit(recovered)?;
        Ok((registry, checked))
    }

    fn commit(&mut self, snapshot: Snapshot) -> Result<()> {
        let directory = self.path.parent().context("missing registry directory")?;
        let mut temp = tempfile::NamedTempFile::new_in(directory)?;
        let bytes = serde_json::to_vec(&snapshot)?;
        ensure!(
            bytes.len() < 4 * 1024 * 1024,
            "session registry exceeds 4 MiB; archive sessions before continuing"
        );
        temp.write_all(&bytes)?;
        temp.write_all(b"\n")?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path)?;
        // Rename is atomic; sync the directory before acknowledging persistence.
        File::open(directory)?.sync_all()?;
        self.snapshot = snapshot;
        Ok(())
    }

    pub fn create(&mut self, spec: SessionSpec) -> Result<Session> {
        self.create_inner(spec, None)
    }

    pub fn retry(&mut self, id: Uuid, acknowledge_prior_effects: bool) -> Result<Session> {
        ensure!(
            acknowledge_prior_effects,
            "owner must acknowledge prior effects before creating a retry"
        );
        let source = self.get(id)?;
        ensure!(
            source.execution.mode != cerynth_agent_api::SessionMode::Observed,
            "observed processes have no owned retry boundary"
        );
        if let Some(existing) = source.execution.retry_session_id {
            return self.get(existing);
        }
        ensure!(
            matches!(source.state, SessionState::Failed | SessionState::Cancelled),
            "only failed/cancelled sessions may be retried"
        );
        ensure!(
            !source.execution.cleanup_pending
                && !source.execution.model_residency_cleanup_pending
                && !source.execution.resource_cleanup_pending
                && !source
                    .execution
                    .resource_allocation
                    .as_ref()
                    .is_some_and(|a| a.active)
                && !source.execution.guard_cleanup_pending
                && !source.execution.gpu_cleanup_pending
                && !source.execution.workspace_preparing,
            "finish all cleanup before retrying"
        );
        ensure!(
            source.execution.operations.iter().all(|op| matches!(
                op.status,
                cerynth_agent_api::OperationStatus::Completed
                    | cerynth_agent_api::OperationStatus::Failed
            )),
            "unknown operation outcome cannot be retried"
        );
        ensure!(
            source
                .execution
                .retry
                .as_ref()
                .is_none_or(|retry| retry.attempt < 3),
            "three-retry limit reached"
        );
        ensure!(
            self.snapshot.sessions.contains_key(&id),
            "archived retry source is read-only"
        );
        self.create_inner(source.spec.clone(), Some(&source))
    }

    // Validate parent constraints and commit retry lineage/source links atomically.
    #[allow(clippy::too_many_lines)]
    fn create_inner(
        &mut self,
        mut spec: SessionSpec,
        retry_source: Option<&Session>,
    ) -> Result<Session> {
        spec.validate().map_err(anyhow::Error::msg)?;
        spec.workspace = spec
            .workspace
            .canonicalize()
            .context("workspace does not exist")?;
        ensure!(spec.workspace.is_dir(), "workspace must be a directory");
        ensure!(
            self.snapshot.sessions.len() < MAX_SESSIONS,
            "registry full (prototype limit: 128 sessions)"
        );
        if let Some(parent_id) = spec.runtime.parent_id {
            let parent = self.get(parent_id)?;
            if let (Some(child), Some(parent)) = (&spec.runtime.guard, &parent.spec.runtime.guard) {
                ensure!(
                    child
                        .remotes
                        .iter()
                        .all(|(name, url)| parent.remotes.get(name) == Some(url)),
                    "child cannot change owner-approved Git remote targets"
                );
            }
            if spec.runtime.budget.cpu_quota_micros_per_second.is_none() {
                spec.runtime.budget.cpu_quota_micros_per_second =
                    parent.spec.runtime.budget.cpu_quota_micros_per_second;
            }
            if let Some(limits) = &parent.spec.runtime.budget.platform {
                spec.runtime
                    .budget
                    .platform
                    .get_or_insert_with(Default::default)
                    .inherit_from(limits);
                spec.runtime.budget.validate().map_err(anyhow::Error::msg)?;
            }
            if spec.runtime.budget.workspace_max_bytes.is_none() {
                spec.runtime.budget.workspace_max_bytes =
                    parent.spec.runtime.budget.workspace_max_bytes;
            }
            ensure!(!parent.state.is_terminal(), "parent session is terminal");
            ensure!(
                spec.runtime.guard.is_some() == parent.spec.runtime.guard.is_some(),
                "child cannot change the Guard provider"
            );
            ensure!(
                spec.runtime.project == parent.spec.runtime.project,
                "child project differs from parent"
            );
            ensure!(
                spec.runtime
                    .capabilities
                    .is_subset(&parent.spec.runtime.capabilities),
                "child capability escalation"
            );
            ensure!(
                spec.runtime
                    .message_peers
                    .is_subset(&parent.spec.runtime.message_peers),
                "child message-sharing escalation"
            );
            ensure!(
                spec.runtime.budget.fits_within(&parent.spec.runtime.budget),
                "child budget exceeds parent"
            );
            ensure!(
                spec.runtime.network == cerynth_agent_api::NetworkPolicy::Deny
                    || spec.runtime.network == parent.spec.runtime.network,
                "child network escalation"
            );
            ensure!(
                spec.runtime.isolation == cerynth_agent_api::IsolationPolicy::Required
                    || spec.runtime.isolation == parent.spec.runtime.isolation,
                "child isolation escalation"
            );
            ensure!(
                !parent.spec.runtime.require_cgroup || spec.runtime.require_cgroup,
                "child cgroup escalation"
            );
            let children: Vec<_> = self
                .snapshot
                .sessions
                .values()
                .filter(|s| s.spec.runtime.parent_id == Some(parent_id) && !s.state.is_terminal())
                .collect();
            ensure!(
                children.len() < parent.spec.runtime.budget.max_subagents as usize,
                "parent subagent limit reached"
            );
            let memory = children
                .iter()
                .try_fold(spec.runtime.budget.memory_max_bytes, |total, child| {
                    total.checked_add(child.spec.runtime.budget.memory_max_bytes)
                });
            ensure!(
                memory.is_some_and(|total| total <= parent.spec.runtime.budget.memory_max_bytes),
                "sum of child memory reservations exceeds parent"
            );
            let gpu = children
                .iter()
                .try_fold(spec.runtime.budget.gpu_memory_bytes, |total, child| {
                    total.checked_add(child.spec.runtime.budget.gpu_memory_bytes)
                });
            ensure!(
                gpu.is_some_and(|total| total <= parent.spec.runtime.budget.gpu_memory_bytes),
                "sum of child GPU reservations exceeds parent"
            );
            let concurrency: u64 = children
                .iter()
                .map(|s| u64::from(s.spec.runtime.budget.inference_concurrency))
                .sum();
            ensure!(
                concurrency + u64::from(spec.runtime.budget.inference_concurrency)
                    <= u64::from(parent.spec.runtime.budget.inference_concurrency),
                "child inference reservations exceed parent"
            );
        }
        let timestamp = now_ms();
        let mut session = Session {
            id: Uuid::new_v4(),
            owner_uid: nix::unistd::getuid().as_raw(),
            spec,
            state: SessionState::Created,
            boundary: ExecutionBoundary::UnenforcedPrototype,
            created_at_ms: timestamp,
            updated_at_ms: timestamp,
            events: vec![SessionEvent {
                sequence: 1,
                timestamp_ms: timestamp,
                state: SessionState::Created,
                reason: "session_created".into(),
            }],
            execution: ExecutionRecord::default(),
        };
        let mut snapshot = self.snapshot.clone();
        if let Some(source) = retry_source {
            session.execution.retry = Some(cerynth_agent_api::RetryLineage {
                root_session_id: source
                    .execution
                    .retry
                    .as_ref()
                    .map_or(source.id, |retry| retry.root_session_id),
                previous_session_id: source.id,
                attempt: source
                    .execution
                    .retry
                    .as_ref()
                    .map_or(1, |retry| retry.attempt + 1),
            });
            snapshot
                .sessions
                .get_mut(&source.id)
                .context("retry source disappeared")?
                .execution
                .retry_session_id = Some(session.id);
        }
        snapshot.sessions.insert(session.id, session.clone());
        self.commit(snapshot)?;
        Ok(session)
    }

    pub fn get(&self, id: Uuid) -> Result<Session> {
        if let Some(session) = self.snapshot.sessions.get(&id) {
            return Ok(session.clone());
        }
        let path = self
            .path
            .parent()
            .context("registry directory missing")?
            .join("archive")
            .join(format!("{id}.json"));
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(path)
            .context("unknown session")?;
        ensure!(
            file.metadata()?.is_file() && file.metadata()?.len() < 4 * 1024 * 1024,
            "invalid archived session"
        );
        let session: Session = serde_json::from_reader(file)?;
        ensure!(
            session.id == id && session.owner_uid == nix::unistd::getuid().as_raw(),
            "archived identity mismatch"
        );
        Ok(session)
    }

    pub fn operation_owner(&self, id: Uuid) -> Option<Uuid> {
        self.snapshot
            .archived_operations
            .get(&id)
            .copied()
            .or_else(|| {
                self.snapshot
                    .sessions
                    .values()
                    .find(|s| {
                        s.execution
                            .operations
                            .iter()
                            .any(|operation| operation.id == id)
                    })
                    .map(|s| s.id)
            })
    }

    pub fn archive(&mut self, id: Uuid) -> Result<()> {
        let session = self
            .snapshot
            .sessions
            .get(&id)
            .context("session is unknown or already archived")?;
        ensure!(
            session.state.is_terminal()
                && !session.execution.model_residency_cleanup_pending
                && !session.execution.cleanup_pending
                && !session.execution.guard_cleanup_pending
                && !session.execution.resource_cleanup_pending
                && !session
                    .execution
                    .resource_allocation
                    .as_ref()
                    .is_some_and(|a| a.active)
                && !session.execution.gpu_cleanup_pending
                && !session.execution.workspace_preparing,
            "session cleanup is not complete"
        );
        ensure!(
            !self
                .snapshot
                .sessions
                .values()
                .any(|s| s.spec.runtime.parent_id == Some(id)),
            "archive children before their parent"
        );
        ensure!(
            self.snapshot.archived_operations.len() + session.execution.operations.len() <= 4096,
            "archive operation ledger is full"
        );
        let directory = self
            .path
            .parent()
            .context("registry directory missing")?
            .join("archive");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)?;
        let mut archive = tempfile::NamedTempFile::new_in(&directory)?;
        serde_json::to_writer(&mut archive, session)?;
        archive.as_file().sync_all()?;
        archive.persist(directory.join(format!("{id}.json")))?;
        File::open(directory)?.sync_all()?;
        let mut snapshot = self.snapshot.clone();
        for operation in &session.execution.operations {
            snapshot.archived_operations.insert(operation.id, id);
        }
        snapshot.sessions.remove(&id);
        self.commit(snapshot)
    }

    pub fn list(&self) -> Vec<Session> {
        self.snapshot.sessions.values().cloned().collect()
    }

    fn change(session: &mut Session, state: SessionState, reason: &str) -> Result<()> {
        ensure!(
            session.state.can_transition_to(state),
            "invalid session state transition"
        );
        session.state = state;
        session.updated_at_ms = now_ms();
        Self::record_event(session, reason);
        Ok(())
    }

    pub fn record_event(session: &mut Session, reason: &str) {
        session.updated_at_ms = now_ms();
        let sequence = session.events.last().map_or(1, |event| event.sequence + 1);
        if session.events.len() >= 1024 {
            session.events.remove(0);
        }
        session.events.push(SessionEvent {
            sequence,
            timestamp_ms: session.updated_at_ms,
            state: session.state,
            reason: reason.into(),
        });
    }

    pub fn update(
        &mut self,
        id: Uuid,
        apply: impl FnOnce(&mut Session) -> Result<()>,
    ) -> Result<Session> {
        let mut snapshot = self.snapshot.clone();
        let session = snapshot.sessions.get_mut(&id).context("unknown session")?;
        apply(session)?;
        session.updated_at_ms = now_ms();
        let result = session.clone();
        self.commit(snapshot)?;
        Ok(result)
    }

    pub fn transition(&mut self, id: Uuid, state: SessionState, reason: &str) -> Result<Session> {
        let mut snapshot = self.snapshot.clone();
        let session = snapshot.sessions.get_mut(&id).context("unknown session")?;
        Self::change(session, state, reason)?;
        let result = session.clone();
        self.commit(snapshot)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cerynth_agent_api::{HarnessKind, ModelConfig};
    use std::os::unix::fs::PermissionsExt;

    fn private_tempdir() -> Result<tempfile::TempDir> {
        Ok(tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()?)
    }

    #[test]
    fn retries_require_known_clean_outcomes_and_remain_bounded_across_restarts() -> Result<()> {
        let directory = private_tempdir()?;
        let mut registry = Registry::open(directory.path())?;
        let root = registry.create(spec(directory.path()))?;
        assert!(registry.retry(root.id, true).is_err());
        registry.transition(root.id, SessionState::Failed, "fixture_failed")?;
        assert!(registry.retry(root.id, false).is_err());
        registry.update(root.id, |session| {
            session
                .execution
                .operations
                .push(cerynth_agent_api::OperationRecord {
                    id: Uuid::new_v4(),
                    fingerprint: "public-fixture".into(),
                    status: cerynth_agent_api::OperationStatus::Unknown,
                });
            Ok(())
        })?;
        assert!(registry.retry(root.id, true).is_err());
        registry.update(root.id, |s| {
            s.execution.operations[0].status = cerynth_agent_api::OperationStatus::Failed;
            s.execution.guard_cleanup_pending = true;
            Ok(())
        })?;
        assert!(registry.retry(root.id, true).is_err());
        registry.update(root.id, |s| {
            s.execution.guard_cleanup_pending = false;
            Ok(())
        })?;
        let retry = registry.retry(root.id, true)?;
        assert_eq!(retry.state, SessionState::Created);
        assert!(retry.execution.process.is_none() && retry.execution.operations.is_empty());
        assert_eq!(retry.execution.retry.as_ref().unwrap().attempt, 1);
        assert_eq!(registry.retry(root.id, true)?.id, retry.id);
        drop(registry);
        let mut registry = Registry::open(directory.path())?;
        assert_eq!(registry.retry(root.id, true)?.id, retry.id);
        let mut previous = retry;
        for attempt in 2..=3 {
            registry.transition(previous.id, SessionState::Failed, "fixture_failed")?;
            previous = registry.retry(previous.id, true)?;
            let lineage = previous.execution.retry.as_ref().unwrap();
            assert_eq!(lineage.attempt, attempt);
            assert_eq!(lineage.root_session_id, root.id);
        }
        registry.transition(previous.id, SessionState::Failed, "fixture_failed")?;
        assert!(registry.retry(previous.id, true).is_err());
        Ok(())
    }

    pub fn spec(directory: &Path) -> SessionSpec {
        SessionSpec {
            name: "probe".into(),
            harness: HarnessKind::InferenceProbe,
            workspace: directory.into(),
            model: ModelConfig {
                name: "test-model".into(),
                mode: cerynth_agent_api::ModelMode::Local,
                endpoint_id: None,
                cloud_consent: false,
                disable_thinking: false,
            },
            runtime: cerynth_agent_api::RuntimeSpec::default(),
        }
    }

    #[test]
    fn persistence_lock_restart_recovery_and_terminal_protection() -> Result<()> {
        let dir = private_tempdir()?;
        let mut registry = Registry::open(dir.path())?;
        assert!(Registry::open(dir.path()).is_err());
        let session = registry.create(spec(dir.path()))?;
        registry.transition(session.id, SessionState::Preparing, "test")?;
        drop(registry);
        let mut registry = Registry::open(dir.path())?;
        let recovered = registry.get(session.id)?;
        assert_eq!(recovered.state, SessionState::Failed);
        assert_eq!(recovered.events.last().unwrap().reason, "daemon_restart");
        assert!(
            registry
                .transition(session.id, SessionState::Running, "invalid")
                .is_err()
        );
        assert_eq!(registry.get(session.id)?.state, SessionState::Failed);
        assert_eq!(
            fs::metadata(dir.path().join("sessions.json"))?.mode() & 0o777,
            0o600
        );
        Ok(())
    }

    #[test]
    fn corrupt_state_is_preserved_and_rejected() -> Result<()> {
        let dir = private_tempdir()?;
        let registry = Registry::open(dir.path())?;
        drop(registry);
        fs::write(dir.path().join("sessions.json"), b"broken")?;
        assert!(Registry::open(dir.path()).is_err());
        assert_eq!(fs::read(dir.path().join("sessions.json"))?, b"broken");
        Ok(())
    }
}
