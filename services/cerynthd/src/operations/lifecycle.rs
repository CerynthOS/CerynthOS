//! Operation lifecycle state machine.
//!
//! PENDING -> RUNNING -> COMPLETED / FAILED / INTERRUPTED
//! INTERRUPTED -> RESUMABLE -> RUNNING

use anyhow::Result;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Lifecycle state of an operation execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OperationState {
    Pending,
    Running,
    Completed,
    Failed,
    Interrupted,
    Resumable,
    Cancelled,
}

impl OperationState {
    /// Valid states from which an operation may be resumed.
    #[must_use]
    pub fn is_resumable(&self) -> bool {
        matches!(self, Self::Interrupted | Self::Resumable)
    }

    /// Whether this is a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Validate a transition. Returns an error if the transition is illegal.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn validate_transition(self, next: Self) -> Result<(), LifecycleError> {
        let valid = matches!(
            (self, next),
            (Self::Pending, Self::Running | Self::Cancelled)
                | (
                    Self::Running,
                    Self::Completed | Self::Failed | Self::Interrupted | Self::Cancelled
                )
                | (Self::Interrupted, Self::Resumable)
                | (Self::Resumable, Self::Running | Self::Cancelled)
        );
        if valid {
            Ok(())
        } else {
            Err(LifecycleError::IllegalTransition(self, next))
        }
    }
}

/// Errors from lifecycle operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleError {
    IllegalTransition(OperationState, OperationState),
    NotResumable(OperationState),
    TerminalState(OperationState),
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IllegalTransition(from, to) => {
                write!(f, "illegal state transition {from:?} -> {to:?}")
            }
            Self::NotResumable(state) => write!(f, "state {state:?} is not resumable"),
            Self::TerminalState(state) => write!(f, "state {state:?} is terminal"),
        }
    }
}

impl std::error::Error for LifecycleError {}

/// A single execution instance of a registered operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// Unique identifier for this execution instance.
    pub instance_id: Uuid,
    /// ID of the registered operation.
    pub operation_id: String,
    /// Current lifecycle state.
    pub state: OperationState,
    /// Serialized input parameters.
    pub input: serde_json::Value,
    /// Serialized output, if completed.
    pub output: Option<serde_json::Value>,
    /// Failure reason, if failed.
    pub error: Option<String>,
    /// Number of execution attempts (incremented on resume).
    pub attempts: u32,
    /// Idempotency key; used to avoid repeating destructive actions.
    pub idempotency_key: Option<String>,
    /// Creation timestamp (Unix epoch milliseconds).
    pub created_at_ms: u64,
    /// Last update timestamp (Unix epoch milliseconds).
    pub updated_at_ms: u64,
}

impl ExecutionRecord {
    /// Create a new pending execution.
    #[must_use]
    pub fn new(
        operation_id: impl Into<String>,
        input: serde_json::Value,
        idempotency_key: Option<String>,
    ) -> Self {
        Self {
            instance_id: Uuid::new_v4(),
            operation_id: operation_id.into(),
            state: OperationState::Pending,
            input,
            output: None,
            error: None,
            attempts: 0,
            idempotency_key,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        }
    }

    /// Transition to a new state, validating the transition.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn transition(&mut self, next: OperationState) -> Result<(), LifecycleError> {
        self.state.validate_transition(next)?;
        self.state = next;
        self.updated_at_ms = now_ms();
        Ok(())
    }

    /// Mark as interrupted (crash, daemon restart, etc.).
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn interrupt(&mut self) -> Result<(), LifecycleError> {
        self.transition(OperationState::Interrupted)
    }

    /// Mark as resumable after diagnostics confirm it is safe to resume.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn mark_resumable(&mut self) -> Result<(), LifecycleError> {
        if !self.state.is_resumable() {
            return Err(LifecycleError::NotResumable(self.state));
        }
        self.transition(OperationState::Resumable)
    }

    /// Prepare for a resumed attempt: increment attempts and mark running.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn begin_resume(&mut self) -> Result<(), LifecycleError> {
        if !self.state.is_resumable() {
            return Err(LifecycleError::NotResumable(self.state));
        }
        self.attempts = self.attempts.saturating_add(1);
        self.transition(OperationState::Running)
    }

    /// Mark as completed with output.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn complete(&mut self, output: serde_json::Value) -> Result<(), LifecycleError> {
        if self.state != OperationState::Running {
            return Err(LifecycleError::IllegalTransition(
                self.state,
                OperationState::Completed,
            ));
        }
        self.output = Some(output);
        self.error = None;
        self.transition(OperationState::Completed)
    }

    /// Mark as failed with an error.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn fail(&mut self, error: impl Into<String>) -> Result<(), LifecycleError> {
        if self.state != OperationState::Running {
            return Err(LifecycleError::IllegalTransition(
                self.state,
                OperationState::Failed,
            ));
        }
        self.error = Some(error.into());
        self.transition(OperationState::Failed)
    }
}

/// A representation of a state transition for auditing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateTransition {
    pub from: OperationState,
    pub to: OperationState,
    pub at_ms: u64,
    pub reason: Option<String>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_state_flow() {
        let mut record = ExecutionRecord::new("test.op", serde_json::json!({}), None);
        assert_eq!(record.state, OperationState::Pending);
        record.transition(OperationState::Running).unwrap();
        record.complete(serde_json::json!({"ok": true})).unwrap();
        assert_eq!(record.state, OperationState::Completed);
        assert!(record.output.is_some());
    }

    #[test]
    fn interrupt_and_resume_flow() {
        let mut record = ExecutionRecord::new("test.op", serde_json::json!({}), None);
        record.transition(OperationState::Running).unwrap();
        record.interrupt().unwrap();
        assert!(record.state.is_resumable());
        record.mark_resumable().unwrap();
        record.begin_resume().unwrap();
        assert_eq!(record.state, OperationState::Running);
        assert_eq!(record.attempts, 1);
        record.complete(serde_json::json!({})).unwrap();
    }

    #[test]
    fn illegal_transitions_rejected() {
        let mut record = ExecutionRecord::new("test.op", serde_json::json!({}), None);
        assert!(record.complete(serde_json::json!({})).is_err());
        assert!(record.transition(OperationState::Completed).is_err());
        record.transition(OperationState::Running).unwrap();
        record.interrupt().unwrap();
        assert!(record.complete(serde_json::json!({})).is_err());
    }

    #[test]
    fn terminal_states_reject_transitions() {
        let mut record = ExecutionRecord::new("test.op", serde_json::json!({}), None);
        record.transition(OperationState::Running).unwrap();
        record.complete(serde_json::json!({})).unwrap();
        assert!(record.transition(OperationState::Running).is_err());
        assert!(record.state.is_terminal());
    }

    #[test]
    fn failure_flow() {
        let mut record = ExecutionRecord::new("test.op", serde_json::json!({}), None);
        record.transition(OperationState::Running).unwrap();
        record.fail("boom").unwrap();
        assert_eq!(record.state, OperationState::Failed);
        assert_eq!(record.error.as_deref(), Some("boom"));
    }
}
