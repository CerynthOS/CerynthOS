//! Owner-selected semantic scheduling within existing CPU weight ceilings.
use anyhow::{Result, ensure};
use cerynth_agent_api::WorkloadPhase;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Demand {
    pub id: Uuid,
    pub parent: Option<Uuid>,
    pub unit: String,
    pub phase: WorkloadPhase,
    pub ceiling: u32,
}

#[must_use]
pub fn preference(phase: WorkloadPhase, ceiling: u32, pressure: bool) -> u32 {
    let percent = match phase {
        WorkloadPhase::Tool => 100,
        WorkloadPhase::Cleanup => 80,
        WorkloadPhase::Inference => 70,
        WorkloadPhase::Compile => 60,
        WorkloadPhase::Batch => {
            if pressure {
                10
            } else {
                40
            }
        }
        WorkloadPhase::Idle => {
            if pressure {
                5
            } else {
                10
            }
        }
        WorkloadPhase::ApprovalWait => 5,
    };
    (ceiling.saturating_mul(percent) / 100).clamp(1, ceiling.max(1))
}

/// # Errors
/// Rejects cycles, missing ancestors or invalid owner ceilings.
pub fn plan(demands: &[Demand], pressure: bool) -> Result<BTreeMap<Uuid, u32>> {
    ensure!(demands.len() <= 128, "phase policy registry limit exceeded");
    let nodes: BTreeMap<_, _> = demands.iter().map(|node| (node.id, node)).collect();
    ensure!(
        nodes.len() == demands.len()
            && demands
                .iter()
                .all(|node| (1..=10_000).contains(&node.ceiling)),
        "invalid phase policy identity or ceiling"
    );
    let mut result: BTreeMap<_, _> = demands
        .iter()
        .map(|node| (node.id, preference(node.phase, node.ceiling, pressure)))
        .collect();
    for node in demands {
        let requested = result[&node.id];
        let mut parent = node.parent;
        let mut seen = BTreeSet::from([node.id]);
        while let Some(id) = parent {
            ensure!(seen.insert(id), "cyclic phase scheduling hierarchy");
            let ancestor = nodes
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("active phase ancestor missing"))?;
            let weight = result
                .get_mut(&id)
                .ok_or_else(|| anyhow::anyhow!("phase ancestor weight missing"))?;
            *weight = (*weight).max(requested.min(ancestor.ceiling));
            parent = ancestor.parent;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interactive_child_keeps_ancestor_runnable_without_exceeding_grants() -> Result<()> {
        let parent = Demand {
            id: Uuid::new_v4(),
            parent: None,
            unit: String::new(),
            phase: WorkloadPhase::ApprovalWait,
            ceiling: 200,
        };
        let child = Demand {
            id: Uuid::new_v4(),
            parent: Some(parent.id),
            unit: String::new(),
            phase: WorkloadPhase::Tool,
            ceiling: 100,
        };
        let result = plan(&[parent.clone(), child.clone()], false)?;
        assert_eq!(result[&parent.id], 100);
        assert_eq!(result[&child.id], 100);
        assert!(
            preference(WorkloadPhase::Batch, 100, true)
                < preference(WorkloadPhase::Batch, 100, false)
        );
        let mut cycle = parent.clone();
        cycle.parent = Some(child.id);
        assert!(plan(&[cycle, child], false).is_err());
        Ok(())
    }
}
