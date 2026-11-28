//! Resource enforcement via cgroups v2.
//!
//! Applies CPU quotas and memory limits to registered agents using the
//! existing `cerynth-platform-resources` enforcement layer. Reads back what
//! the kernel actually accepted; never claims enforcement without read-back.

use anyhow::{Context, Result, ensure};
use cerynth_platform_resources::{CpuMax, Enforcement, ResourceLimits, SessionCgroup};
use serde::{Deserialize, Serialize};

use crate::resources::registration::AgentRegistration;

/// The result of applying resource limits to an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnforcementResult {
    /// Whether a CPU quota was enforced by the kernel.
    pub cpu_enforced: bool,
    /// Whether a memory limit was enforced by the kernel.
    pub memory_enforced: bool,
    /// Human-readable notes about unsupported or adjusted limits.
    pub notes: Vec<String>,
}

impl EnforcementResult {
    /// Whether all requested limits were enforced.
    #[must_use]
    pub fn fully_enforced(&self) -> bool {
        self.cpu_enforced && self.memory_enforced && self.notes.is_empty()
    }
}

/// Apply the limits from a registration to its cgroup.
///
/// Opens the cgroup at the registered path (does not create it) so an
/// unprivileged caller can use a delegated systemd scope. Creation is a
/// separate, privileged operation.
///
/// # Errors
/// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
pub fn apply_limits(registration: &AgentRegistration) -> Result<EnforcementResult> {
    super::registration::validate_kernel_boundary(registration)
        .context("invalid kernel registration")?;
    ensure!(
        registration.cpu_quota_us.is_none()
            || std::fs::read_to_string("/sys/kernel/sched_ext/state")
                .unwrap_or_default()
                .trim()
                != "enabled",
        "native CPU bandwidth is unqualified under an active sched_ext scheduler"
    );

    let session = SessionCgroup::open(registration.cgroup_path.clone(), registration.agent_id)?;

    let limits = ResourceLimits {
        cpu_max: registration
            .cpu_quota_us
            .map(|quota| CpuMax::new(quota, registration.cpu_period_us)),
        memory_max_bytes: registration.memory_limit_bytes,
        ..ResourceLimits::default()
    };

    let applied = session.apply(&limits)?;
    let mut notes = Vec::new();

    let cpu_enforced = applied.cpu_max.as_ref().is_some_and(|l| {
        if l.enforcement == Enforcement::Enforced {
            true
        } else {
            if let Some(note) = &l.note {
                notes.push(format!("cpu.max: {note}"));
            }
            false
        }
    });
    let memory_enforced = applied.memory_max_bytes.as_ref().is_some_and(|l| {
        if l.enforcement == Enforcement::Enforced {
            true
        } else {
            if let Some(note) = &l.note {
                notes.push(format!("memory.max: {note}"));
            }
            false
        }
    });

    // If no limits were requested, treat as trivially enforced.
    let cpu_enforced = cpu_enforced || registration.cpu_quota_us.is_none();
    let memory_enforced = memory_enforced || registration.memory_limit_bytes.is_none();

    Ok(EnforcementResult {
        cpu_enforced,
        memory_enforced,
        notes,
    })
}

/// Attach the registered PID to its cgroup.
///
/// # Errors
/// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
pub fn attach_process(registration: &AgentRegistration, pid: i32) -> Result<()> {
    super::registration::validate_kernel_boundary(registration)?;
    ensure!(pid > 0, "attach requires a positive process ID");
    let session = SessionCgroup::open(registration.cgroup_path.clone(), registration.agent_id)?;
    session.attach(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforcement_result_semantics() {
        let mut result = EnforcementResult {
            cpu_enforced: true,
            memory_enforced: true,
            notes: vec![],
        };
        assert!(result.fully_enforced());
        result.notes.push("warning".into());
        assert!(!result.fully_enforced());
        result.notes.clear();
        result.memory_enforced = false;
        assert!(!result.fully_enforced());
    }
}
