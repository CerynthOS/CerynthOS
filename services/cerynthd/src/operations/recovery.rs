//! Crash and restart recovery for interrupted operations.
//!
//! Scans persisted execution records on daemon startup and marks running
//! operations as interrupted, then prepares resumable ones for retry. Completed
//! operations are never accidentally re-executed.

use anyhow::Result;
use uuid::Uuid;

use crate::operations::lifecycle::{ExecutionRecord, OperationState};
use crate::operations::persistence::OperationStateStore;

/// Outcome of a recovery scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Records that were RUNNING and are now INTERRUPTED.
    pub interrupted: Vec<Uuid>,
    /// Records that are now RESUMABLE.
    pub resumable: Vec<Uuid>,
    /// Records that were already terminal and were left untouched.
    pub terminal_preserved: usize,
}

/// Recover operation state after a crash or daemon restart.
pub fn recover(
    store: &mut OperationStateStore,
    registry: &crate::operations::registry::OperationRegistry,
) -> Result<RecoveryReport> {
    let mut report = RecoveryReport {
        interrupted: vec![],
        resumable: vec![],
        terminal_preserved: 0,
    };

    let ids: Vec<Uuid> = store.list().iter().map(|r| r.instance_id).collect();
    for id in ids {
        let record = store.get_mut(id).expect("record was just listed");
        if record.state.is_terminal() {
            report.terminal_preserved += 1;
            continue;
        }
        if record.state == OperationState::Running {
            record
                .transition(OperationState::Interrupted)
                .expect("running -> interrupted is always valid");
            report.interrupted.push(id);
        }
        if record.state.is_resumable() {
            let can_resume = registry
                .get(&record.operation_id)
                .is_some_and(|op| op.resumable);
            if can_resume {
                record
                    .mark_resumable()
                    .expect("interrupted -> resumable is always valid");
                report.resumable.push(id);
            }
        }
    }
    store.save()?;
    Ok(report)
}

/// Prepare a specific record for resume on a fresh executor.
pub fn prepare_resume(
    record: &mut ExecutionRecord,
) -> Result<(), crate::operations::lifecycle::LifecycleError> {
    if !record.state.is_resumable() {
        return Err(crate::operations::lifecycle::LifecycleError::NotResumable(
            record.state,
        ));
    }
    record.mark_resumable()?;
    record.begin_resume()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::registry::{Operation, OperationRegistry};

    fn setup() -> (OperationRegistry, OperationStateStore) {
        let mut registry = OperationRegistry::new();
        registry
            .register(Operation::new("resumable.op", "R", "R", false, true))
            .unwrap();
        registry
            .register(Operation::new("nonresumable.op", "N", "N", false, false))
            .unwrap();
        (registry, OperationStateStore::new())
    }

    #[test]
    fn running_becomes_interrupted_then_resumable() {
        let (registry, mut store) = setup();
        let mut record = ExecutionRecord::new("resumable.op", serde_json::json!({}), None);
        record.transition(OperationState::Running).unwrap();
        let id = record.instance_id;
        store.put(record);

        let report = recover(&mut store, &registry).unwrap();
        assert_eq!(report.interrupted, vec![id]);
        assert_eq!(report.resumable, vec![id]);
        assert_eq!(store.get(id).unwrap().state, OperationState::Resumable);
    }

    #[test]
    fn completed_is_preserved() {
        let (registry, mut store) = setup();
        let mut record = ExecutionRecord::new("resumable.op", serde_json::json!({}), None);
        record.transition(OperationState::Running).unwrap();
        record.complete(serde_json::json!({"done": true})).unwrap();
        let id = record.instance_id;
        store.put(record);

        let report = recover(&mut store, &registry).unwrap();
        assert_eq!(report.terminal_preserved, 1);
        assert!(report.interrupted.is_empty());
        assert_eq!(store.get(id).unwrap().state, OperationState::Completed);
    }

    #[test]
    fn nonresumable_operation_not_marked_resumable() {
        let (registry, mut store) = setup();
        let mut record = ExecutionRecord::new("nonresumable.op", serde_json::json!({}), None);
        record.transition(OperationState::Running).unwrap();
        let id = record.instance_id;
        store.put(record);

        let report = recover(&mut store, &registry).unwrap();
        assert_eq!(report.interrupted, vec![id]);
        assert!(report.resumable.is_empty());
        assert_eq!(store.get(id).unwrap().state, OperationState::Interrupted);
    }
}
