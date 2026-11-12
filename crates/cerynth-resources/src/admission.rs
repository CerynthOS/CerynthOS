//! Owner policy for managed CPU/RAM reservations. Agents never supply capacity.
use anyhow::{Result, ensure};
use cerynth_agent_api::ResourceBudget;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capacity {
    pub memory_bytes: u64,
    pub cpu_quota_micros_per_second: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Accept { granted: ResourceBudget },
    Reduce { granted: ResourceBudget },
    Queue { reason: String },
    Reject { reason: String },
}

impl Decision {
    #[must_use]
    pub fn granted(&self) -> Option<&ResourceBudget> {
        match self {
            Self::Accept { granted } | Self::Reduce { granted } => Some(granted),
            Self::Queue { .. } | Self::Reject { .. } => None,
        }
    }
}

/// Minimum is an explicit owner authorization for reductions, never a model claim.
/// # Errors
/// Rejects malformed budgets, capacity and a minimum broader than the request.
pub fn decide(
    requested: &ResourceBudget,
    minimum: Option<&ResourceBudget>,
    capacity: &Capacity,
    reserved: &Capacity,
    pressure_blocks_new_work: bool,
) -> Result<Decision> {
    requested.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        capacity.memory_bytes > 0 && capacity.cpu_quota_micros_per_second >= 10_000,
        "invalid owner capacity"
    );
    let minimum = minimum.unwrap_or(requested);
    minimum.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        minimum.fits_within(requested),
        "minimum exceeds owner request"
    );
    let minimum_cpu = minimum.cpu_quota_micros_per_second.unwrap_or(0);
    if minimum.memory_max_bytes > capacity.memory_bytes
        || minimum_cpu > capacity.cpu_quota_micros_per_second
    {
        return Ok(Decision::Reject {
            reason: "minimum_exceeds_physical_policy_capacity".into(),
        });
    }
    if pressure_blocks_new_work {
        return Ok(Decision::Queue {
            reason: "resource_pressure".into(),
        });
    }
    let memory = capacity.memory_bytes.saturating_sub(reserved.memory_bytes);
    let cpu = capacity
        .cpu_quota_micros_per_second
        .saturating_sub(reserved.cpu_quota_micros_per_second);
    if memory < minimum.memory_max_bytes || cpu < minimum_cpu {
        return Ok(Decision::Queue {
            reason: "active_reservations".into(),
        });
    }
    let mut granted = requested.clone();
    granted.memory_max_bytes = granted.memory_max_bytes.min(memory);
    granted.memory_high_bytes = granted.memory_high_bytes.min(granted.memory_max_bytes);
    if let Some(requested_cpu) = requested.cpu_quota_micros_per_second {
        granted.cpu_quota_micros_per_second = Some(requested_cpu.min(cpu));
    }
    // All noncapacity grants remain the exact owner request. A reduction may
    // never silently lower below the owner's minimum, including memory.high.
    if !minimum.fits_within(&granted) {
        return Ok(Decision::Queue {
            reason: "minimum_cannot_be_preserved".into(),
        });
    }
    Ok(if granted == *requested {
        Decision::Accept { granted }
    } else {
        Decision::Reduce { granted }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_reduces_queues_and_rejects_without_expanding_authority() -> Result<()> {
        let request = ResourceBudget {
            cpu_quota_micros_per_second: Some(2_000_000),
            memory_max_bytes: 4_000_000_000,
            memory_high_bytes: 3_000_000_000,
            ..ResourceBudget::default()
        };
        let capacity = Capacity {
            memory_bytes: 6_000_000_000,
            cpu_quota_micros_per_second: 4_000_000,
        };
        let empty = Capacity {
            memory_bytes: 0,
            cpu_quota_micros_per_second: 0,
        };
        assert!(matches!(
            decide(&request, None, &capacity, &empty, false)?,
            Decision::Accept { .. }
        ));
        let held = Capacity {
            memory_bytes: 3_000_000_000,
            cpu_quota_micros_per_second: 3_000_000,
        };
        assert!(matches!(
            decide(&request, None, &capacity, &held, false)?,
            Decision::Queue { .. }
        ));
        let minimum = ResourceBudget {
            cpu_quota_micros_per_second: Some(500_000),
            memory_max_bytes: 2_000_000_000,
            memory_high_bytes: 1_000_000_000,
            ..request.clone()
        };
        let granted = decide(&request, Some(&minimum), &capacity, &held, false)?;
        assert!(matches!(granted, Decision::Reduce { .. }));
        let granted = granted.granted().unwrap();
        assert_eq!(granted.memory_max_bytes, 3_000_000_000);
        assert_eq!(granted.cpu_quota_micros_per_second, Some(1_000_000));
        assert!(granted.fits_within(&request) && minimum.fits_within(granted));
        assert!(matches!(
            decide(&request, None, &capacity, &empty, true)?,
            Decision::Queue { .. }
        ));
        let impossible = Capacity {
            memory_bytes: 1_000_000_000,
            ..capacity
        };
        assert!(matches!(
            decide(&request, None, &impossible, &empty, false)?,
            Decision::Reject { .. }
        ));
        assert!(decide(&minimum, Some(&request), &impossible, &empty, false).is_err());
        Ok(())
    }
}
