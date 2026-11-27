//! Central machine-wide accounting service.
//!
//! Collects actual CPU, memory, and process consumption from kernel-maintained
//! interfaces for every registered agent. No agent-reported values are used.

use anyhow::{Context, Result};
use cerynth_platform_resources::SessionCgroup;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use crate::resources::cpu::CpuTopology;
use crate::resources::memory::MachineMemory;
use crate::resources::process::ProcessInfo;
use crate::resources::registration::{AgentRegistration, ExecutionState, RegistrationStore};

/// A single agent's accounting snapshot, read from the kernel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentUsage {
    pub agent_id: Uuid,
    pub cgroup_path: PathBuf,
    pub state: ExecutionState,
    /// Cumulative CPU usage in microseconds from cgroup v2 `cpu.stat`.
    pub cpu_usage_usec: u64,
    /// Number of CPU periods throttled by the kernel.
    pub cpu_throttled_periods: u64,
    /// Current memory usage in bytes from `memory.current`.
    pub memory_current_bytes: u64,
    /// Kernel memory limit in bytes (`memory.max`), if set.
    pub memory_limit_bytes: Option<u64>,
    /// Process IDs in this agent's cgroup.
    pub pids: Vec<i32>,
    /// Number of OOM kills in this cgroup.
    pub oom_kills: u64,
    /// Whether the cgroup is currently populated.
    pub populated: bool,
    /// Whether the data was read successfully from the cgroup.
    pub kernel_backed: bool,
}

/// Machine-wide usage summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineUsage {
    /// Unix epoch milliseconds when this snapshot was taken.
    pub observed_at_ms: u64,
    /// Per-agent accounting snapshots.
    pub agents: Vec<AgentUsage>,
    /// Machine memory information.
    pub memory: MachineMemory,
    /// CPU topology.
    pub topology: CpuTopology,
    /// Agents whose cgroup could not be read.
    pub read_errors: Vec<String>,
}

impl MachineUsage {
    /// Aggregate CPU usage across all agents.
    #[must_use]
    pub fn total_cpu_usec(&self) -> u64 {
        self.agents.iter().map(|a| a.cpu_usage_usec).sum()
    }

    /// Aggregate current memory usage across all agents.
    #[must_use]
    pub fn total_memory_bytes(&self) -> u64 {
        self.agents.iter().map(|a| a.memory_current_bytes).sum()
    }

    /// Number of agents with a live process.
    #[must_use]
    pub fn running_agents(&self) -> usize {
        self.agents
            .iter()
            .filter(|a| a.state == ExecutionState::Running && a.populated)
            .count()
    }
}

/// Central accounting service.
pub struct AccountingService {
    registrations: RegistrationStore,
    state_path: Option<PathBuf>,
}

impl AccountingService {
    /// Create an accounting service from an existing registration store.
    #[must_use]
    pub fn new(registrations: RegistrationStore) -> Self {
        Self {
            registrations,
            state_path: None,
        }
    }

    /// Create a service that persists registrations to a file.
    pub fn with_persistence(registrations: RegistrationStore, state_path: PathBuf) -> Self {
        Self {
            registrations,
            state_path: Some(state_path),
        }
    }

    /// Access the registration store.
    #[must_use]
    pub fn registrations(&self) -> &RegistrationStore {
        &self.registrations
    }

    /// Mutable access to the registration store.
    pub fn registrations_mut(&mut self) -> &mut RegistrationStore {
        &mut self.registrations
    }

    /// Persist registrations if a state path is configured.
    pub fn persist(&self) -> Result<()> {
        if let Some(path) = &self.state_path {
            self.registrations.save(path)?;
        }
        Ok(())
    }

    /// Collect a machine-wide accounting snapshot.
    pub fn snapshot(&self) -> Result<MachineUsage> {
        let now_ms = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .context("system clock before epoch")?
                .as_millis(),
        )
        .unwrap_or(0);

        let mut agents = Vec::new();
        let mut read_errors = Vec::new();

        for registration in self.registrations.list() {
            match read_agent_usage(registration) {
                Ok(usage) => agents.push(usage),
                Err(err) => {
                    // A dead or inaccessible agent must not corrupt others.
                    read_errors.push(format!("{}: {err}", registration.agent_id));
                    agents.push(unavailable_usage(registration));
                }
            }
        }

        Ok(MachineUsage {
            observed_at_ms: now_ms,
            agents,
            memory: MachineMemory::read().unwrap_or_else(|_| MachineMemory {
                total_bytes: None,
                free_bytes: None,
                available_bytes: None,
                swap_total_bytes: None,
                swap_free_bytes: None,
            }),
            topology: CpuTopology::discover().unwrap_or_else(|_| CpuTopology {
                logical_cpus: vec![],
                core_ids: vec![],
                node_ids: vec![],
                thread_siblings: vec![],
            }),
            read_errors,
        })
    }

    /// Get the usage for one agent by ID.
    pub fn agent_usage(&self, agent_id: Uuid) -> Result<AgentUsage> {
        let registration = self
            .registrations
            .get(agent_id)
            .context("agent not registered")?;
        read_agent_usage(registration)
    }

    /// Get process info for a registered agent's primary PID.
    pub fn process_info(&self, agent_id: Uuid) -> Result<Option<ProcessInfo>> {
        let registration = self
            .registrations
            .get(agent_id)
            .context("agent not registered")?;
        match registration.pid {
            Some(pid) => ProcessInfo::read(pid),
            None => Ok(None),
        }
    }
}

/// Read kernel accounting for one agent from its cgroup.
fn read_agent_usage(registration: &AgentRegistration) -> Result<AgentUsage> {
    let session = SessionCgroup::open(registration.cgroup_path.clone(), registration.agent_id)?;
    let usage = session.usage()?;

    Ok(AgentUsage {
        agent_id: registration.agent_id,
        cgroup_path: registration.cgroup_path.clone(),
        state: registration.state,
        cpu_usage_usec: usage.cpu.usage_usec,
        cpu_throttled_periods: usage.cpu.nr_throttled,
        memory_current_bytes: usage.memory.current_bytes,
        memory_limit_bytes: usage.memory.max_bytes,
        pids: usage.pids,
        oom_kills: usage.memory.events.oom_kill,
        populated: usage.populated,
        kernel_backed: true,
    })
}

fn unavailable_usage(registration: &AgentRegistration) -> AgentUsage {
    AgentUsage {
        agent_id: registration.agent_id,
        cgroup_path: registration.cgroup_path.clone(),
        state: registration.state,
        cpu_usage_usec: 0,
        cpu_throttled_periods: 0,
        memory_current_bytes: 0,
        memory_limit_bytes: None,
        pids: vec![],
        oom_kills: 0,
        populated: false,
        kernel_backed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_cgroup() -> PathBuf {
        let root = std::env::temp_dir().join(format!("acct-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("cgroup.procs"), "").unwrap();
        std::fs::write(root.join("cgroup.events"), "populated 0\n").unwrap();
        std::fs::write(root.join("cpu.stat"), "usage_usec 12345\nuser_usec 10000\nsystem_usec 2345\nnr_periods 10\nnr_throttled 2\nthrottled_usec 500\n").unwrap();
        std::fs::write(root.join("memory.current"), "67890\n").unwrap();
        std::fs::write(root.join("memory.max"), "104857600\n").unwrap();
        std::fs::write(
            root.join("memory.events"),
            "low 0\nhigh 0\nmax 0\noom 0\noom_kill 0\n",
        )
        .unwrap();
        std::fs::write(root.join("pids.current"), "0\n").unwrap();
        std::fs::write(root.join("pids.max"), "100\n").unwrap();
        root
    }

    #[test]
    fn snapshot_reads_kernel_accounting() {
        let cgroup = fixture_cgroup();
        let mut regs = RegistrationStore::new();
        let id = Uuid::new_v4();
        let mut reg = AgentRegistration::new(id, cgroup.clone());
        reg.state = ExecutionState::Running;
        regs.register(reg).unwrap();

        let service = AccountingService::new(regs);
        let snapshot = service.snapshot().unwrap();
        assert_eq!(snapshot.agents.len(), 1);
        let agent = &snapshot.agents[0];
        assert_eq!(agent.agent_id, id);
        assert_eq!(agent.cpu_usage_usec, 12345);
        assert_eq!(agent.cpu_throttled_periods, 2);
        assert_eq!(agent.memory_current_bytes, 67890);
        assert_eq!(agent.memory_limit_bytes, Some(104857600));
        assert!(agent.kernel_backed);
        assert_eq!(snapshot.total_cpu_usec(), 12345);
        assert_eq!(snapshot.total_memory_bytes(), 67890);
        assert_eq!(snapshot.running_agents(), 0); // populated=0 in fixture

        let _ = std::fs::remove_dir_all(&cgroup);
    }

    #[test]
    fn failed_agent_does_not_corrupt_others() {
        let good = fixture_cgroup();
        let mut regs = RegistrationStore::new();
        let good_id = Uuid::new_v4();
        regs.register(AgentRegistration::new(good_id, good.clone()))
            .unwrap();
        let bad_id = Uuid::new_v4();
        regs.register(AgentRegistration::new(bad_id, "/definitely/not/a/cgroup"))
            .unwrap();

        let service = AccountingService::new(regs);
        let snapshot = service.snapshot().unwrap();
        assert_eq!(snapshot.agents.len(), 2);
        assert_eq!(snapshot.read_errors.len(), 1);
        let good_usage = snapshot
            .agents
            .iter()
            .find(|a| a.agent_id == good_id)
            .unwrap();
        assert!(good_usage.kernel_backed);
        let bad_usage = snapshot
            .agents
            .iter()
            .find(|a| a.agent_id == bad_id)
            .unwrap();
        assert!(!bad_usage.kernel_backed);

        let _ = std::fs::remove_dir_all(&good);
    }
}
