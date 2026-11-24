//! Adapter to Person 1's pinned `ResourceBudget` v1 implementation.
//! systemd remains the sole writer; the upstream API validates and reads usage.
use anyhow::{Result, ensure};
use cerynth_agent_api::{Session, WorkloadPhase};
use cerynth_platform_resources::{
    AppliedLimit, AppliedLimits, CpuMax, Enforcement, PriorityClass, ResourceLimits, SessionCgroup,
};
pub use cerynth_platform_resources::{BudgetStatus, ResourceBudget, ResourceUsage};
use serde::Serialize;
use std::path::Path;

pub const REVISION: &str = "648c149be39f5ea8df612b3266857472976238eb";

#[derive(Serialize)]
pub struct Snapshot {
    pub source_revision: &'static str,
    pub writer: &'static str,
    pub request: ResourceBudget,
    pub status: BudgetStatus,
    pub usage: ResourceUsage,
}

/// # Errors
/// Rejects requests that either the agent or platform contract cannot represent.
pub fn request(session: &Session) -> Result<ResourceBudget> {
    session
        .spec
        .runtime
        .budget
        .validate()
        .map_err(anyhow::Error::msg)?;
    let budget = &session.spec.runtime.budget;
    let phase = match session.execution.phase {
        WorkloadPhase::Inference => PriorityClass::Inference,
        WorkloadPhase::Compile | WorkloadPhase::Batch => PriorityClass::BatchCompute,
        WorkloadPhase::Idle | WorkloadPhase::ApprovalWait => PriorityClass::Background,
        _ => PriorityClass::Balanced,
    };
    let request = ResourceBudget {
        schema_version: 1,
        session_id: session.id,
        priority_class: phase,
        limits: ResourceLimits {
            cpu_weight: Some(u64::from(budget.cpu_weight)),
            cpu_max: budget
                .cpu_quota_micros_per_second
                .map(|quota| CpuMax::new(quota / 10, 100_000)),
            memory_high_bytes: Some(budget.memory_high_bytes),
            memory_max_bytes: Some(budget.memory_max_bytes),
            io_weight: budget
                .platform
                .as_ref()
                .and_then(|limits| limits.io_weight)
                .map(u64::from),
            pids_max: Some(u64::from(budget.tasks_max)),
        },
    };
    request.validate()?;
    Ok(request)
}

fn observed<T: PartialEq>(requested: T, applied: Option<T>, written: bool) -> AppliedLimit<T> {
    let enforcement = if written && applied.as_ref() == Some(&requested) {
        Enforcement::Enforced
    } else if applied.is_some() {
        Enforcement::Observed
    } else {
        Enforcement::Unsupported
    };
    AppliedLimit {
        requested,
        applied,
        enforcement,
        note: (enforcement != Enforcement::Enforced)
            .then(|| "not verified as applied by the owned systemd writer".into()),
    }
}

/// Read through the actual pinned platform API without invoking its direct writes.
/// # Errors
/// Rejects unsafe paths or malformed/missing mandatory kernel data.
pub fn snapshot(session: &Session, path: &Path) -> Result<Snapshot> {
    ensure!(
        path.starts_with("/sys/fs/cgroup")
            && !path
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir)),
        "invalid owned cgroup path"
    );
    let request = request(session)?;
    let usage = SessionCgroup::open(path, session.id)?.usage()?;
    let written = session
        .execution
        .resource_allocation
        .as_ref()
        .is_some_and(|a| a.active && a.kernel_enforced);
    let read = |name: &str| std::fs::read_to_string(path.join(name)).ok();
    let integer = |name: &str| read(name).and_then(|s| s.trim().parse::<u64>().ok());
    let limits = AppliedLimits {
        cpu_weight: request
            .limits
            .cpu_weight
            .map(|v| observed(v, integer("cpu.weight"), written)),
        cpu_max: request
            .limits
            .cpu_max
            .map(|v| observed(v, read("cpu.max").and_then(|s| s.parse().ok()), written)),
        memory_high_bytes: request
            .limits
            .memory_high_bytes
            .map(|v| observed(v, usage.memory.high_bytes, written)),
        memory_max_bytes: request
            .limits
            .memory_max_bytes
            .map(|v| observed(v, usage.memory.max_bytes, written)),
        pids_max: request
            .limits
            .pids_max
            .map(|v| observed(v, usage.pids_usage.max, written)),
        // A weight is a configured preference, not a device throughput guarantee.
        io_weight: request.limits.io_weight.map(|weight| {
            observed(
                weight,
                read("io.weight").and_then(|text| {
                    text.lines().find_map(|line| {
                        line.strip_prefix("default ")
                            .and_then(|value| value.parse().ok())
                    })
                }),
                false,
            )
        }),
    };
    let status = BudgetStatus {
        schema_version: 1,
        session_id: session.id,
        cgroup_path: path.to_string_lossy().into_owned(),
        limits,
    };
    Ok(Snapshot {
        source_revision: REVISION,
        writer: "systemd_dbus_only",
        request,
        status,
        usage,
    })
}

/// Combine actual platform cgroup PSI with machine headroom; neither is an agent report.
pub fn pressure(sample: &mut crate::pressure::Sample, usage: &ResourceUsage) {
    fn maximum(target: &mut Option<f64>, value: Option<f64>) {
        if let Some(value) = value {
            *target = Some(target.unwrap_or(0.0).max(value));
        }
    }
    maximum(
        &mut sample.memory_some_avg10,
        usage.pressure.memory.map(|p| p.some.avg10),
    );
    maximum(
        &mut sample.memory_full_avg10,
        usage.pressure.memory.map(|p| p.full.avg10),
    );
    maximum(
        &mut sample.cpu_some_avg10,
        usage.pressure.cpu.map(|p| p.some.avg10),
    );
    maximum(
        &mut sample.io_full_avg10,
        usage.pressure.io.map(|p| p.full.avg10),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn platform_cgroup_pressure_blocks_admission_even_with_healthy_machine_headroom() {
        use cerynth_platform_resources::{Pressure, PressureLine, PressureValues};
        let mut sample = crate::pressure::Sample {
            version: 1,
            observed_at_ms: 1000,
            available_memory_bytes: Some(1024_u64.pow(3)),
            memory_some_avg10: Some(0.0),
            memory_full_avg10: Some(0.0),
            cpu_some_avg10: Some(0.0),
            io_full_avg10: Some(0.0),
        };
        assert!(!sample.blocks_new_work(1000));
        let usage = ResourceUsage {
            schema_version: 1,
            session_id: uuid::Uuid::new_v4(),
            cgroup_path: "/sys/fs/cgroup/public-fixture".into(),
            populated: true,
            pids: vec![],
            cpu: cerynth_platform_resources::CpuUsage::default(),
            memory: cerynth_platform_resources::MemoryUsage::default(),
            pids_usage: cerynth_platform_resources::PidsUsage::default(),
            io: vec![],
            pressure: Pressure {
                memory: Some(PressureLine {
                    full: PressureValues {
                        avg10: 8.0,
                        ..PressureValues::default()
                    },
                    ..PressureLine::default()
                }),
                ..Pressure::default()
            },
        };
        pressure(&mut sample, &usage);
        assert!(sample.blocks_new_work(1000));
        assert_eq!(sample.memory_full_avg10, Some(8.0));
    }

    #[test]
    fn observation_never_claims_enforcement_without_writer_evidence() {
        assert_eq!(
            observed(42, Some(42), false).enforcement,
            Enforcement::Observed
        );
        assert_eq!(
            observed(42, Some(41), true).enforcement,
            Enforcement::Observed
        );
        assert_eq!(
            observed(42, Some(42), true).enforcement,
            Enforcement::Enforced
        );
        assert_eq!(
            observed(42, None, true).enforcement,
            Enforcement::Unsupported
        );
    }
}
