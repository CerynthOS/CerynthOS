//! Operation execution with lifecycle tracking.
//!
//! Tracks execution state through the lifecycle and persists transitions.

use anyhow::Result;
use uuid::Uuid;

use crate::operations::lifecycle::{ExecutionRecord, LifecycleError, OperationState};
use crate::operations::persistence::OperationStateStore;
use crate::operations::registry::OperationRegistry;

/// A handler for a registered operation.
pub trait ExecutionHandler: Send + Sync {
    /// Execute the operation with the given input.
    ///
    /// Returns the output on success or an error message on failure.
    ///
    /// # Errors
    /// Returns the handler's error without claiming successful execution.
    fn execute(&self, input: &serde_json::Value) -> Result<serde_json::Value, String>;

    /// Resume an interrupted operation with prior partial output.
    ///
    /// # Errors
    /// Returns an error when resumption is unsupported or fails.
    fn resume(
        &self,
        _input: &serde_json::Value,
        _prior_output: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        Err("resume not implemented for this operation".to_string())
    }
}

/// Executes registered operations and tracks their lifecycle.
pub struct Executor {
    registry: OperationRegistry,
    store: OperationStateStore,
    persistence_healthy: bool,
}

impl Executor {
    /// Create an executor with the given registry and state store.
    #[must_use]
    pub fn new(registry: OperationRegistry, store: OperationStateStore) -> Self {
        Self {
            registry,
            store,
            persistence_healthy: true,
        }
    }

    /// Access the registry.
    #[must_use]
    pub fn registry(&self) -> &OperationRegistry {
        &self.registry
    }

    /// Access the state store.
    #[must_use]
    pub fn store(&self) -> &OperationStateStore {
        &self.store
    }

    /// Mutable access to the state store.
    pub fn store_mut(&mut self) -> &mut OperationStateStore {
        &mut self.store
    }

    /// Start a new execution of an operation.
    ///
    /// Checks authorization, validates the operation exists, and transitions
    /// the record to RUNNING. Persistence is attempted after every transition.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn start(
        &mut self,
        operation_id: &str,
        input: serde_json::Value,
        is_root: bool,
        idempotency_key: Option<String>,
    ) -> Result<Uuid, ExecutorError> {
        let operation = self
            .registry
            .authorize(operation_id, is_root)
            .map_err(|e| ExecutorError::Authorization(e.to_string()))?;

        if !self.persistence_healthy {
            return Err(ExecutorError::Persistence(
                "state persistence previously failed; restart/reconcile before execution".into(),
            ));
        }
        if let Some(key) = &idempotency_key {
            if key.is_empty() || key.len() > 256 {
                return Err(ExecutorError::IdempotencyConflict);
            }
            if let Some(existing) = self
                .store
                .list()
                .into_iter()
                .find(|r| r.idempotency_key.as_ref() == Some(key))
            {
                if existing.operation_id != operation.id || existing.input != input {
                    return Err(ExecutorError::IdempotencyConflict);
                }
                return Ok(existing.instance_id);
            }
        }

        let mut record = ExecutionRecord::new(&operation.id, input, idempotency_key);
        record
            .transition(OperationState::Running)
            .map_err(ExecutorError::Lifecycle)?;
        let id = record.instance_id;
        self.store.put(record);
        self.persist()?;
        Ok(id)
    }

    /// Execute a started operation with a handler.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn execute(
        &mut self,
        instance_id: Uuid,
        handler: &dyn ExecutionHandler,
    ) -> Result<serde_json::Value, ExecutorError> {
        let record = self
            .store
            .get(instance_id)
            .ok_or(ExecutorError::NotFound(instance_id))?;
        if record.state != OperationState::Running {
            return Err(ExecutorError::Lifecycle(LifecycleError::IllegalTransition(
                record.state,
                OperationState::Running,
            )));
        }
        let input = record.input.clone();
        self.persist()?;
        let outcome = handler.execute(&input);
        self.finish(instance_id, outcome)
    }

    fn persist(&mut self) -> Result<(), ExecutorError> {
        if !self.persistence_healthy {
            return Err(ExecutorError::Persistence(
                "state durability unavailable".into(),
            ));
        }
        if let Err(error) = self.store.save() {
            self.persistence_healthy = false;
            return Err(ExecutorError::Persistence(error.to_string()));
        }
        Ok(())
    }

    fn finish(
        &mut self,
        id: Uuid,
        outcome: Result<serde_json::Value, String>,
    ) -> Result<serde_json::Value, ExecutorError> {
        let record = self.store.get_mut(id).ok_or(ExecutorError::NotFound(id))?;
        match &outcome {
            Ok(output) => record
                .complete(output.clone())
                .map_err(ExecutorError::Lifecycle)?,
            Err(error) => record
                .fail(error.clone())
                .map_err(ExecutorError::Lifecycle)?,
        }
        self.persist()?;
        outcome.map_err(ExecutorError::ExecutionFailed)
    }

    /// Mark a running operation as interrupted (e.g. daemon shutdown).
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn interrupt(&mut self, instance_id: Uuid) -> Result<(), ExecutorError> {
        let record = self
            .store
            .get_mut(instance_id)
            .ok_or(ExecutorError::NotFound(instance_id))?;
        record.interrupt().map_err(ExecutorError::Lifecycle)?;
        self.persist()?;
        Ok(())
    }

    /// Resume an interrupted operation with a handler.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn resume(
        &mut self,
        instance_id: Uuid,
        handler: &dyn ExecutionHandler,
    ) -> Result<serde_json::Value, ExecutorError> {
        let record = self
            .store
            .get_mut(instance_id)
            .ok_or(ExecutorError::NotFound(instance_id))?;
        if !record.state.is_resumable() {
            return Err(ExecutorError::Lifecycle(LifecycleError::NotResumable(
                record.state,
            )));
        }
        let operation = self.registry.get(&record.operation_id).ok_or_else(|| {
            ExecutorError::Authorization(format!("unknown operation {}", record.operation_id))
        })?;
        if !operation.resumable {
            return Err(ExecutorError::NotResumableOperation(
                record.operation_id.clone(),
            ));
        }

        record.mark_resumable().map_err(ExecutorError::Lifecycle)?;
        record.begin_resume().map_err(ExecutorError::Lifecycle)?;

        let input = record.input.clone();
        let prior_output = record.output.clone();
        self.persist()?;
        let outcome = handler.resume(&input, prior_output.as_ref());
        self.finish(instance_id, outcome)
    }
}

/// Errors from the executor.
#[derive(Debug)]
pub enum ExecutorError {
    Authorization(String),
    Lifecycle(LifecycleError),
    NotFound(Uuid),
    Persistence(String),
    IdempotencyConflict,
    ExecutionFailed(String),
    NotResumableOperation(String),
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Persistence(error) => write!(f, "operation state persistence failed: {error}"),
            Self::IdempotencyConflict => {
                write!(f, "idempotency key must bind identical operation details")
            }
            Self::Authorization(msg) => write!(f, "authorization failed: {msg}"),
            Self::Lifecycle(err) => write!(f, "lifecycle error: {err}"),
            Self::NotFound(id) => write!(f, "execution {id} not found"),
            Self::ExecutionFailed(msg) => write!(f, "execution failed: {msg}"),
            Self::NotResumableOperation(id) => {
                write!(f, "operation {id} does not support resumption")
            }
        }
    }
}

impl std::error::Error for ExecutorError {}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoHandler;
    impl ExecutionHandler for EchoHandler {
        fn execute(&self, input: &serde_json::Value) -> Result<serde_json::Value, String> {
            Ok(input.clone())
        }
        fn resume(
            &self,
            input: &serde_json::Value,
            _prior: Option<&serde_json::Value>,
        ) -> Result<serde_json::Value, String> {
            Ok(input.clone())
        }
    }

    struct FailHandler;
    impl ExecutionHandler for FailHandler {
        fn execute(&self, _input: &serde_json::Value) -> Result<serde_json::Value, String> {
            Err("boom".to_string())
        }
    }

    fn setup() -> Executor {
        let mut registry = OperationRegistry::new();
        registry
            .register(crate::operations::registry::Operation::new(
                "test.op",
                "Test",
                "Test operation",
                false,
                true,
            ))
            .unwrap();
        Executor::new(registry, OperationStateStore::new())
    }

    #[test]
    fn running_idempotency_key_is_bound_to_exact_input() {
        let mut executor = setup();
        let input = serde_json::json!({"target":"first"});
        let first = executor
            .start("test.op", input.clone(), false, Some("one".into()))
            .unwrap();
        assert_eq!(
            executor
                .start("test.op", input, false, Some("one".into()))
                .unwrap(),
            first
        );
        assert!(matches!(
            executor.start(
                "test.op",
                serde_json::json!({"target":"other"}),
                false,
                Some("one".into())
            ),
            Err(ExecutorError::IdempotencyConflict)
        ));
        assert_eq!(executor.store().list().len(), 1);
    }

    #[test]
    fn failed_durable_start_cannot_execute_a_handler() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Counter(AtomicUsize);
        impl ExecutionHandler for Counter {
            fn execute(&self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({}))
            }
        }
        let root = std::env::temp_dir().join(format!("durable-start-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let blocked = root.join("not-directory");
        std::fs::write(&blocked, b"existing file").unwrap();
        let mut executor = setup();
        executor.store = OperationStateStore::with_path(blocked.join("state.json"));
        assert!(matches!(
            executor.start("test.op", serde_json::json!({}), false, None),
            Err(ExecutorError::Persistence(_))
        ));
        let id = executor.store().list()[0].instance_id;
        let counter = Counter(AtomicUsize::new(0));
        assert!(matches!(
            executor.execute(id, &counter),
            Err(ExecutorError::Persistence(_))
        ));
        assert_eq!(counter.0.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn start_execute_complete() {
        let mut executor = setup();
        let id = executor
            .start("test.op", serde_json::json!({"x": 1}), false, None)
            .unwrap();
        let output = executor.execute(id, &EchoHandler).unwrap();
        assert_eq!(output, serde_json::json!({"x": 1}));
        assert_eq!(
            executor.store().get(id).unwrap().state,
            OperationState::Completed
        );
    }

    #[test]
    fn unauthorized_rejected() {
        let mut registry = OperationRegistry::new();
        registry
            .register(crate::operations::registry::Operation::new(
                "admin.op", "Admin", "Admin op", true, true,
            ))
            .unwrap();
        let mut executor = Executor::new(registry, OperationStateStore::new());
        assert!(
            executor
                .start("admin.op", serde_json::json!({}), false, None)
                .is_err()
        );
    }

    #[test]
    fn failure_is_recorded() {
        let mut executor = setup();
        let id = executor
            .start("test.op", serde_json::json!({}), false, None)
            .unwrap();
        assert!(executor.execute(id, &FailHandler).is_err());
        assert_eq!(
            executor.store().get(id).unwrap().state,
            OperationState::Failed
        );
        assert_eq!(
            executor.store().get(id).unwrap().error.as_deref(),
            Some("boom")
        );
    }

    #[test]
    fn interrupt_and_resume() {
        let mut executor = setup();
        let id = executor
            .start("test.op", serde_json::json!({"y": 2}), false, None)
            .unwrap();
        executor.interrupt(id).unwrap();
        assert_eq!(
            executor.store().get(id).unwrap().state,
            OperationState::Interrupted
        );
        let output = executor.resume(id, &EchoHandler).unwrap();
        assert_eq!(output, serde_json::json!({"y": 2}));
        assert_eq!(
            executor.store().get(id).unwrap().state,
            OperationState::Completed
        );
        assert_eq!(executor.store().get(id).unwrap().attempts, 1);
    }

    #[test]
    fn idempotency_key_prevents_duplicate() {
        let mut executor = setup();
        let id1 = executor
            .start("test.op", serde_json::json!({}), false, Some("key1".into()))
            .unwrap();
        executor.execute(id1, &EchoHandler).unwrap();
        let id2 = executor
            .start("test.op", serde_json::json!({}), false, Some("key1".into()))
            .unwrap();
        assert_eq!(id1, id2);
    }
}
