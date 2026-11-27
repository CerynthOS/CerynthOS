//! Durable persistence for operation execution records.
//!
//! Uses atomic file replacement so a crash during a write cannot corrupt the
//! state. Records are keyed by execution instance ID.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::operations::lifecycle::ExecutionRecord;

/// Persistent state store for operation executions.
#[derive(Debug, Clone, Default)]
pub struct OperationStateStore {
    records: BTreeMap<Uuid, ExecutionRecord>,
    path: Option<PathBuf>,
}

impl OperationStateStore {
    /// Create an in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a store backed by a file.
    #[must_use]
    pub fn with_path(path: PathBuf) -> Self {
        Self {
            records: BTreeMap::new(),
            path: Some(path),
        }
    }

    /// Insert or replace a record.
    pub fn put(&mut self, record: ExecutionRecord) {
        self.records.insert(record.instance_id, record);
    }

    /// Get a record by instance ID.
    #[must_use]
    pub fn get(&self, id: Uuid) -> Option<&ExecutionRecord> {
        self.records.get(&id)
    }

    /// Get a mutable record by instance ID.
    pub fn get_mut(&mut self, id: Uuid) -> Option<&mut ExecutionRecord> {
        self.records.get_mut(&id)
    }

    /// Remove a record.
    pub fn remove(&mut self, id: Uuid) -> Option<ExecutionRecord> {
        self.records.remove(&id)
    }

    /// List all records.
    #[must_use]
    pub fn list(&self) -> Vec<&ExecutionRecord> {
        self.records.values().collect()
    }

    /// Persist to the configured file atomically.
    ///
    /// An in-memory store (no path) is a no-op: there is nothing durable to
    /// write. This keeps recovery and executor logic uniform.
    pub fn save(&self) -> Result<()> {
        if let Some(path) = &self.path {
            save_to(path, &self.records)?;
        }
        Ok(())
    }

    /// Persist to an explicit path atomically.
    pub fn save_to(path: &Path) -> Result<()> {
        save_to(path, &Self::new().records)
    }

    /// Load records from a file.
    pub fn load(path: PathBuf) -> Result<Self> {
        let data = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let records: BTreeMap<Uuid, ExecutionRecord> =
            serde_json::from_slice(&data).with_context(|| format!("parse {}", path.display()))?;
        for (id, record) in &records {
            anyhow::ensure!(
                *id == record.instance_id,
                "record key mismatch in {}",
                path.display()
            );
        }
        Ok(Self {
            records,
            path: Some(path),
        })
    }
}

fn save_to(path: &Path, records: &BTreeMap<Uuid, ExecutionRecord>) -> Result<()> {
    let data = serde_json::to_vec_pretty(records).with_context(|| "serialize operation records")?;
    let parent = path.parent().context("path has no parent directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &data).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("commit {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::lifecycle::OperationState;

    #[test]
    fn put_get_remove() {
        let mut store = OperationStateStore::new();
        let mut record = ExecutionRecord::new("test.op", serde_json::json!({}), None);
        record.transition(OperationState::Running).unwrap();
        let id = record.instance_id;
        store.put(record);
        assert!(store.get(id).is_some());
        assert_eq!(store.get(id).unwrap().state, OperationState::Running);
        store.remove(id);
        assert!(store.get(id).is_none());
    }

    #[test]
    fn persist_and_load() {
        let dir = std::env::temp_dir().join(format!("ops-persist-{}", Uuid::new_v4()));
        let path = dir.join("operations.json");
        let mut store = OperationStateStore::with_path(path.clone());
        let mut record = ExecutionRecord::new("test.op", serde_json::json!({"x": 1}), None);
        record.transition(OperationState::Running).unwrap();
        record.interrupt().unwrap();
        store.put(record.clone());
        store.save().unwrap();

        let loaded = OperationStateStore::load(path).unwrap();
        let loaded_record = loaded.get(record.instance_id).unwrap();
        assert_eq!(loaded_record.state, OperationState::Interrupted);
        assert_eq!(loaded_record.input, serde_json::json!({"x": 1}));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
