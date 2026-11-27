//! Agent resource registration: identity, cgroup, and execution state.
//!
//! Every agent or workload registered with the system has a unique identity
//! backed by a cgroup v2 path. The registration is keyed by agent ID, never by
//! PID alone, so a crash or restart cannot corrupt another agent's accounting.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Current execution state of a registered agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    /// Registered but not yet running.
    Pending,
    /// Actively executing.
    Running,
    /// Temporarily stopped (e.g. awaiting approval).
    Suspended,
    /// Execution terminated normally.
    Completed,
    /// Execution terminated abnormally.
    Failed,
}

/// Errors from registration operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationError {
    /// The agent ID is already registered.
    DuplicateAgent(Uuid),
    /// The registration was not found.
    NotFound(Uuid),
    /// The cgroup path is already claimed by another agent.
    CgroupConflict(Uuid, PathBuf),
    /// The registration is in an invalid state for the requested operation.
    InvalidState(Uuid, ExecutionState),
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateAgent(id) => write!(f, "agent {id} is already registered"),
            Self::NotFound(id) => write!(f, "agent {id} is not registered"),
            Self::CgroupConflict(id, path) => {
                write!(
                    f,
                    "cgroup {} is already claimed by agent {id}",
                    path.display()
                )
            }
            Self::InvalidState(id, state) => {
                write!(f, "agent {id} is in invalid state {state:?}")
            }
        }
    }
}

impl std::error::Error for RegistrationError {}

/// A registered agent's resource identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRegistration {
    /// Unique agent identity.
    pub agent_id: Uuid,
    /// Primary process ID, if the agent has a live process.
    pub pid: Option<i32>,
    /// Process group ID, if known.
    pub process_group: Option<i32>,
    /// Kernel cgroup ID from `cgroup.stat` `cgroup_id`, if available.
    pub cgroup_id: Option<u64>,
    /// Absolute path to the agent's cgroup v2 directory.
    pub cgroup_path: PathBuf,
    /// Requested CPU quota in microseconds (None = no quota).
    pub cpu_quota_us: Option<u64>,
    /// CPU period in microseconds.
    pub cpu_period_us: u64,
    /// Memory limit in bytes (None = no limit).
    pub memory_limit_bytes: Option<u64>,
    /// Current execution state.
    pub state: ExecutionState,
    /// Registration timestamp (Unix epoch milliseconds).
    pub registered_at_ms: u64,
}

impl AgentRegistration {
    /// Create a new pending registration.
    #[must_use]
    pub fn new(agent_id: Uuid, cgroup_path: impl Into<PathBuf>) -> Self {
        Self {
            agent_id,
            pid: None,
            process_group: None,
            cgroup_id: None,
            cgroup_path: cgroup_path.into(),
            cpu_quota_us: None,
            cpu_period_us: 100_000,
            memory_limit_bytes: None,
            state: ExecutionState::Pending,
            registered_at_ms: 0,
        }
    }

    /// Validate internal consistency.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.cgroup_path.is_absolute(),
            "cgroup path must be absolute"
        );
        ensure!(
            !self
                .cgroup_path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
            "cgroup path must not contain parent traversal"
        );
        ensure!(self.cpu_period_us > 0, "CPU period must be positive");
        if let Some(quota) = self.cpu_quota_us {
            ensure!(quota > 0, "CPU quota must be positive");
        }
        if let Some(limit) = self.memory_limit_bytes {
            ensure!(limit > 0, "memory limit must be positive");
        }
        Ok(())
    }
}

/// An in-memory store of registered agents.
#[derive(Debug, Default)]
pub struct RegistrationStore {
    agents: BTreeMap<Uuid, AgentRegistration>,
    cgroup_owners: BTreeMap<PathBuf, Uuid>,
}

impl RegistrationStore {
    /// Create an empty registration store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an agent. Returns an error if the ID or cgroup path is already in use.
    pub fn register(&mut self, registration: AgentRegistration) -> Result<(), RegistrationError> {
        registration.validate().map_err(|_| {
            RegistrationError::InvalidState(registration.agent_id, registration.state)
        })?;

        if self.agents.contains_key(&registration.agent_id) {
            return Err(RegistrationError::DuplicateAgent(registration.agent_id));
        }
        if let Some(owner) = self.cgroup_owners.get(&registration.cgroup_path) {
            return Err(RegistrationError::CgroupConflict(
                *owner,
                registration.cgroup_path.clone(),
            ));
        }

        self.cgroup_owners
            .insert(registration.cgroup_path.clone(), registration.agent_id);
        self.agents.insert(registration.agent_id, registration);
        Ok(())
    }

    /// Get a registration by agent ID.
    #[must_use]
    pub fn get(&self, agent_id: Uuid) -> Option<&AgentRegistration> {
        self.agents.get(&agent_id)
    }

    /// Get a mutable registration by agent ID.
    pub fn get_mut(&mut self, agent_id: Uuid) -> Option<&mut AgentRegistration> {
        self.agents.get_mut(&agent_id)
    }

    /// Find a registration by cgroup path.
    #[must_use]
    pub fn by_cgroup(&self, path: &Path) -> Option<&AgentRegistration> {
        let owner = self.cgroup_owners.get(path)?;
        self.agents.get(owner)
    }

    /// List all registrations.
    #[must_use]
    pub fn list(&self) -> Vec<&AgentRegistration> {
        self.agents.values().collect()
    }

    /// Update the execution state of an agent.
    pub fn set_state(
        &mut self,
        agent_id: Uuid,
        state: ExecutionState,
    ) -> Result<(), RegistrationError> {
        let registration = self
            .agents
            .get_mut(&agent_id)
            .ok_or(RegistrationError::NotFound(agent_id))?;
        registration.state = state;
        Ok(())
    }

    /// Update the process identity of an agent.
    pub fn set_process(
        &mut self,
        agent_id: Uuid,
        pid: i32,
        process_group: i32,
    ) -> Result<(), RegistrationError> {
        let registration = self
            .agents
            .get_mut(&agent_id)
            .ok_or(RegistrationError::NotFound(agent_id))?;
        registration.pid = Some(pid);
        registration.process_group = Some(process_group);
        Ok(())
    }

    /// Remove a registration.
    pub fn unregister(&mut self, agent_id: Uuid) -> Result<AgentRegistration, RegistrationError> {
        let registration = self
            .agents
            .remove(&agent_id)
            .ok_or(RegistrationError::NotFound(agent_id))?;
        self.cgroup_owners.remove(&registration.cgroup_path);
        Ok(registration)
    }

    /// Persist registrations to a JSON file atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        let data = serde_json::to_vec_pretty(&self.agents)
            .with_context(|| "failed to serialize registrations")?;
        let parent = path
            .parent()
            .context("registration path has no parent directory")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &data)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("failed to commit {}", path.display()))?;
        Ok(())
    }

    /// Load registrations from a JSON file.
    pub fn load(path: &Path) -> Result<Self> {
        let data =
            std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
        let agents: BTreeMap<Uuid, AgentRegistration> = serde_json::from_slice(&data)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let mut store = Self::new();
        for (id, registration) in agents {
            ensure!(id == registration.agent_id, "registration key mismatch");
            store
                .register(registration)
                .context("invalid stored registration")?;
        }
        Ok(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_get() {
        let mut store = RegistrationStore::new();
        let id = Uuid::new_v4();
        let reg = AgentRegistration::new(id, "/sys/fs/cgroup/cerynth-session-test");
        store.register(reg).unwrap();
        assert!(store.get(id).is_some());
        assert_eq!(store.get(id).unwrap().state, ExecutionState::Pending);
    }

    #[test]
    fn duplicate_agent_rejected() {
        let mut store = RegistrationStore::new();
        let id = Uuid::new_v4();
        store
            .register(AgentRegistration::new(id, "/sys/fs/cgroup/a"))
            .unwrap();
        assert!(matches!(
            store.register(AgentRegistration::new(id, "/sys/fs/cgroup/b")),
            Err(RegistrationError::DuplicateAgent(_))
        ));
    }

    #[test]
    fn cgroup_conflict_rejected() {
        let mut store = RegistrationStore::new();
        store
            .register(AgentRegistration::new(Uuid::new_v4(), "/sys/fs/cgroup/a"))
            .unwrap();
        assert!(matches!(
            store.register(AgentRegistration::new(Uuid::new_v4(), "/sys/fs/cgroup/a")),
            Err(RegistrationError::CgroupConflict(_, _))
        ));
    }

    #[test]
    fn state_and_process_updates() {
        let mut store = RegistrationStore::new();
        let id = Uuid::new_v4();
        store
            .register(AgentRegistration::new(id, "/sys/fs/cgroup/a"))
            .unwrap();
        store.set_state(id, ExecutionState::Running).unwrap();
        store.set_process(id, 1234, 1234).unwrap();
        let reg = store.get(id).unwrap();
        assert_eq!(reg.state, ExecutionState::Running);
        assert_eq!(reg.pid, Some(1234));
        assert_eq!(reg.process_group, Some(1234));
    }

    #[test]
    fn unregister_releases_cgroup() {
        let mut store = RegistrationStore::new();
        let id = Uuid::new_v4();
        let path = PathBuf::from("/sys/fs/cgroup/a");
        store
            .register(AgentRegistration::new(id, path.clone()))
            .unwrap();
        store.unregister(id).unwrap();
        assert!(store.get(id).is_none());
        store
            .register(AgentRegistration::new(Uuid::new_v4(), path))
            .unwrap();
    }

    #[test]
    fn rejects_relative_cgroup_path() {
        let mut store = RegistrationStore::new();
        let reg = AgentRegistration::new(Uuid::new_v4(), "relative/path");
        assert!(matches!(
            store.register(reg),
            Err(RegistrationError::InvalidState(_, _))
        ));
    }

    #[test]
    fn persist_and_restore() {
        let dir = std::env::temp_dir().join(format!("reg-test-{}", Uuid::new_v4()));
        let path = dir.join("registrations.json");
        let mut store = RegistrationStore::new();
        let id = Uuid::new_v4();
        let mut reg = AgentRegistration::new(id, "/sys/fs/cgroup/a");
        reg.state = ExecutionState::Running;
        reg.pid = Some(42);
        store.register(reg).unwrap();
        store.save(&path).unwrap();

        let loaded = RegistrationStore::load(&path).unwrap();
        assert_eq!(loaded.get(id).unwrap().pid, Some(42));
        assert_eq!(loaded.get(id).unwrap().state, ExecutionState::Running);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
