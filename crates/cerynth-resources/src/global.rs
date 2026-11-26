//! Global request semantics shared with the pinned platform budget contract.
//! This planner consumes an authoritative snapshot; it never applies a grant.
use crate::admission::{self, Capacity, Decision};
use anyhow::{Result, ensure};
use cerynth_agent_api::{ResourceBudget, Session};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Demand {
    pub schema_version: u32,
    pub request_id: Uuid,
    pub session_id: Uuid,
    pub parent_session_id: Option<Uuid>,
    pub project: String,
    pub requested: ResourceBudget,
    pub minimum: Option<ResourceBudget>,
    pub gpu_device_id: Option<String>,
    pub inference_endpoint_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceleratorCapacity {
    pub total_bytes: u64,
    pub observed_free_bytes: Option<u64>,
    pub reserved_bytes: u64,
    pub desktop_reserve_bytes: u64,
    pub cooperative_admission_supported: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerSnapshot {
    pub observed_at_ms: u64,
    pub complete_managed_workload_accounting: bool,
    pub capacity: Capacity,
    pub reserved: Capacity,
    pub parent_authority: Option<ResourceBudget>,
    pub accelerators: BTreeMap<String, AcceleratorCapacity>,
    pub inference_limit: u32,
    pub inference_reserved: u32,
    pub pressure_blocks_new_work: bool,
}
#[derive(Debug, Serialize)]
pub struct Proposal {
    pub schema_version: u32,
    pub request_id: Uuid,
    pub session_id: Uuid,
    #[serde(flatten)]
    pub decision: Decision,
    pub applied: bool,
    pub gpu_enforcement: &'static str,
}

fn queue(reason: &str) -> Decision {
    Decision::Queue {
        reason: reason.into(),
    }
}
fn reject(reason: &str) -> Decision {
    Decision::Reject {
        reason: reason.into(),
    }
}

impl Demand {
    #[must_use]
    pub fn for_session(
        session: &Session,
        minimum: Option<ResourceBudget>,
        device: Option<String>,
    ) -> Self {
        Self {
            schema_version: 1,
            request_id: Uuid::new_v4(),
            session_id: session.id,
            parent_session_id: session.spec.runtime.parent_id,
            project: session.spec.runtime.project.clone(),
            requested: session.spec.runtime.budget.clone(),
            minimum,
            gpu_device_id: device,
            inference_endpoint_id: session.spec.model.endpoint_id.clone(),
        }
    }
}

/// # Errors
/// Rejects malformed demands, stale owner observations and impossible minimums.
pub fn plan(demand: &Demand, owner: &OwnerSnapshot, now: u64) -> Result<Proposal> {
    ensure!(
        demand.schema_version == 1 && !demand.project.is_empty() && demand.project.len() <= 128,
        "invalid global request identity"
    );
    ensure!(
        owner.observed_at_ms <= now.saturating_add(1000)
            && now.saturating_sub(owner.observed_at_ms) <= 5000,
        "stale global owner snapshot"
    );
    demand.requested.validate().map_err(anyhow::Error::msg)?;
    if let Some(parent) = &owner.parent_authority {
        parent.validate().map_err(anyhow::Error::msg)?;
    }
    let minimum = demand.minimum.as_ref().unwrap_or(&demand.requested);
    minimum.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        minimum.fits_within(&demand.requested),
        "minimum exceeds requested authority"
    );
    let decision = if !owner.complete_managed_workload_accounting {
        queue("global_accounting_incomplete")
    } else if demand.parent_session_id.is_some()
        && owner
            .parent_authority
            .as_ref()
            .is_none_or(|p| !demand.requested.fits_within(p))
    {
        reject("parent_authority_missing_or_exceeded")
    } else {
        components(demand, owner, minimum)?
    };
    Ok(Proposal {
        schema_version: 1,
        request_id: demand.request_id,
        session_id: demand.session_id,
        decision,
        applied: false,
        gpu_enforcement: "cooperative_reservation_only",
    })
}

fn components(
    demand: &Demand,
    owner: &OwnerSnapshot,
    minimum: &ResourceBudget,
) -> Result<Decision> {
    let cpu_ram = admission::decide(
        &demand.requested,
        Some(minimum),
        &owner.capacity,
        &owner.reserved,
        owner.pressure_blocks_new_work,
    )?;
    let Some(mut granted) = cpu_ram.granted().cloned() else {
        return Ok(cpu_ram);
    };
    if granted.gpu_memory_bytes > 0 {
        let Some(device) = demand
            .gpu_device_id
            .as_ref()
            .and_then(|d| owner.accelerators.get(d))
        else {
            return Ok(queue("GPU_device_capacity_unknown"));
        };
        if !device.cooperative_admission_supported {
            return Ok(queue("GPU_monitoring_only"));
        }
        let capacity = device
            .total_bytes
            .saturating_sub(device.desktop_reserve_bytes);
        if minimum.gpu_memory_bytes > capacity {
            return Ok(reject("GPU_minimum_exceeds_capacity"));
        }
        let Some(observed) = device.observed_free_bytes else {
            return Ok(queue("GPU_free_memory_unknown"));
        };
        let available = capacity
            .min(observed.saturating_sub(device.desktop_reserve_bytes))
            .saturating_sub(device.reserved_bytes);
        if minimum.gpu_memory_bytes > available {
            return Ok(queue("GPU_reservations_or_pressure"));
        }
        granted.gpu_memory_bytes = granted.gpu_memory_bytes.min(available);
    }
    if minimum.inference_concurrency > owner.inference_limit {
        return Ok(reject("inference_minimum_exceeds_capacity"));
    }
    let slots = owner
        .inference_limit
        .saturating_sub(owner.inference_reserved);
    if minimum.inference_concurrency > slots {
        return Ok(queue("inference_concurrency_reserved"));
    }
    granted.inference_concurrency = granted.inference_concurrency.min(slots);
    ensure!(
        minimum.fits_within(&granted) && granted.fits_within(&demand.requested),
        "proposal would expand authority or violate minimum"
    );
    Ok(if granted == demand.requested {
        Decision::Accept { granted }
    } else {
        Decision::Reduce { granted }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Demand, OwnerSnapshot) {
        let budget = ResourceBudget {
            memory_max_bytes: 256 * 1024 * 1024,
            memory_high_bytes: 128 * 1024 * 1024,
            cpu_quota_micros_per_second: Some(1_000_000),
            gpu_memory_bytes: 1024 * 1024 * 1024,
            inference_concurrency: 4,
            ..ResourceBudget::default()
        };
        let minimum = ResourceBudget {
            gpu_memory_bytes: 256 * 1024 * 1024,
            inference_concurrency: 1,
            ..budget.clone()
        };
        let demand = Demand {
            schema_version: 1,
            request_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            parent_session_id: None,
            project: "fixture".into(),
            requested: budget,
            minimum: Some(minimum),
            gpu_device_id: Some("gpu".into()),
            inference_endpoint_id: Some("local".into()),
        };
        let owner = OwnerSnapshot {
            observed_at_ms: 1000,
            complete_managed_workload_accounting: false,
            capacity: Capacity {
                memory_bytes: 1024 * 1024 * 1024,
                cpu_quota_micros_per_second: 2_000_000,
            },
            reserved: Capacity {
                memory_bytes: 0,
                cpu_quota_micros_per_second: 0,
            },
            parent_authority: None,
            accelerators: BTreeMap::from([(
                "gpu".into(),
                AcceleratorCapacity {
                    total_bytes: 2 * 1024 * 1024 * 1024,
                    observed_free_bytes: Some(2 * 1024 * 1024 * 1024),
                    reserved_bytes: 0,
                    desktop_reserve_bytes: 256 * 1024 * 1024,
                    cooperative_admission_supported: true,
                },
            )]),
            inference_limit: 4,
            inference_reserved: 0,
            pressure_blocks_new_work: false,
        };
        (demand, owner)
    }

    #[test]
    fn all_resource_classes_share_one_bounded_non_authorizing_plan() -> Result<()> {
        let (demand, mut owner) = fixture();
        assert!(matches!(
            plan(&demand, &owner, 1000)?.decision,
            Decision::Queue { .. }
        ));
        owner.complete_managed_workload_accounting = true;
        assert!(matches!(
            plan(&demand, &owner, 1000)?.decision,
            Decision::Accept { .. }
        ));
        owner.inference_reserved = 2;
        let reduced = plan(&demand, &owner, 1000)?;
        assert!(!reduced.applied && matches!(reduced.decision, Decision::Reduce { .. }));
        owner.inference_reserved = 4;
        assert!(matches!(
            plan(&demand, &owner, 1000)?.decision,
            Decision::Queue { .. }
        ));
        owner.inference_limit = 0;
        assert!(matches!(
            plan(&demand, &owner, 1000)?.decision,
            Decision::Reject { .. }
        ));
        assert!(plan(&demand, &owner, 7000).is_err());
        owner.inference_limit = 4;
        owner.inference_reserved = 0;
        let mut child = demand.clone();
        child.parent_session_id = Some(Uuid::new_v4());
        assert!(matches!(
            plan(&child, &owner, 1000)?.decision,
            Decision::Reject { .. }
        ));
        owner.parent_authority = child.minimum.clone();
        assert!(matches!(
            plan(&child, &owner, 1000)?.decision,
            Decision::Reject { .. }
        ));
        owner.parent_authority = Some(child.requested.clone());
        assert!(matches!(
            plan(&child, &owner, 1000)?.decision,
            Decision::Accept { .. }
        ));
        owner
            .accelerators
            .get_mut("gpu")
            .expect("fixture device")
            .observed_free_bytes = Some(512 * 1024 * 1024);
        let limited = plan(&child, &owner, 1000)?;
        assert!(matches!(limited.decision, Decision::Reduce { .. }));
        assert_eq!(
            limited
                .decision
                .granted()
                .expect("reduced grant")
                .gpu_memory_bytes,
            256 * 1024 * 1024
        );
        owner
            .accelerators
            .get_mut("gpu")
            .expect("fixture device")
            .observed_free_bytes = None;
        assert!(matches!(
            plan(&child, &owner, 1000)?.decision,
            Decision::Queue { .. }
        ));
        Ok(())
    }
}
