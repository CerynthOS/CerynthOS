//! Central registry of supported operations.
//!
//! Each operation is described by a unique ID, name, permission requirements,
//! input/output schemas, and resume capability.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Definition of a supported operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    /// Globally unique operation ID (e.g. "storage.map_worktree").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of what the operation does.
    pub description: String,
    /// Whether the operation requires root privileges.
    pub requires_root: bool,
    /// Whether the operation supports resumption after interruption.
    pub resumable: bool,
    /// JSON Schema for input parameters.
    pub input_schema: serde_json::Value,
    /// JSON Schema for expected output.
    pub output_schema: serde_json::Value,
}

impl Operation {
    /// Create a simple operation definition with empty object schemas.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        requires_root: bool,
        resumable: bool,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            requires_root,
            resumable,
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: serde_json::json!({"type": "object"}),
        }
    }

    /// Set custom input/output schemas.
    #[must_use]
    pub fn with_schemas(mut self, input: serde_json::Value, output: serde_json::Value) -> Self {
        self.input_schema = input;
        self.output_schema = output;
        self
    }

    /// Validate the definition.
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.id.is_empty(), "operation ID must not be empty");
        ensure!(
            self.id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'),
            "operation ID contains invalid characters: {}",
            self.id
        );
        ensure!(!self.name.is_empty(), "operation name must not be empty");
        Ok(())
    }
}

/// Errors from registry operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationError {
    DuplicateOperation(String),
    UnknownOperation(String),
    InvalidDefinition(String),
    Unauthorized(String, bool),
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateOperation(id) => write!(f, "operation {id} is already registered"),
            Self::UnknownOperation(id) => write!(f, "unknown operation: {id}"),
            Self::InvalidDefinition(reason) => write!(f, "invalid operation definition: {reason}"),
            Self::Unauthorized(id, root) => {
                write!(f, "operation {id} requires root={root}")
            }
        }
    }
}

impl std::error::Error for RegistrationError {}

/// A registry of supported operations.
#[derive(Debug, Default)]
pub struct OperationRegistry {
    operations: BTreeMap<String, Operation>,
}

impl OperationRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an operation.
    pub fn register(&mut self, operation: Operation) -> Result<(), RegistrationError> {
        operation
            .validate()
            .map_err(|e| RegistrationError::InvalidDefinition(e.to_string()))?;
        if self.operations.contains_key(&operation.id) {
            return Err(RegistrationError::DuplicateOperation(operation.id));
        }
        self.operations.insert(operation.id.clone(), operation);
        Ok(())
    }

    /// Look up an operation by ID.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Operation> {
        self.operations.get(id)
    }

    /// List all registered operations, sorted by ID.
    #[must_use]
    pub fn list(&self) -> Vec<&Operation> {
        self.operations.values().collect()
    }

    /// Check whether an operation can be executed with the given privilege level.
    pub fn authorize(&self, id: &str, is_root: bool) -> Result<&Operation, RegistrationError> {
        let operation = self
            .operations
            .get(id)
            .ok_or_else(|| RegistrationError::UnknownOperation(id.to_string()))?;
        if operation.requires_root && !is_root {
            return Err(RegistrationError::Unauthorized(
                id.to_string(),
                operation.requires_root,
            ));
        }
        Ok(operation)
    }

    /// Number of registered operations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.operations.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Operation {
        Operation::new(
            "storage.map",
            "Map Worktree",
            "Map a worktree to a volume",
            true,
            true,
        )
    }

    #[test]
    fn register_and_lookup() {
        let mut registry = OperationRegistry::new();
        registry.register(sample()).unwrap();
        assert!(registry.get("storage.map").is_some());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn duplicate_rejected() {
        let mut registry = OperationRegistry::new();
        registry.register(sample()).unwrap();
        assert!(matches!(
            registry.register(sample()),
            Err(RegistrationError::DuplicateOperation(_))
        ));
    }

    #[test]
    fn unknown_rejected() {
        let registry = OperationRegistry::new();
        assert!(matches!(
            registry.authorize("nope", true),
            Err(RegistrationError::UnknownOperation(_))
        ));
    }

    #[test]
    fn authorization_check() {
        let mut registry = OperationRegistry::new();
        registry.register(sample()).unwrap();
        assert!(authorize_helper(&registry, "storage.map", false).is_err());
        assert!(registry.authorize("storage.map", true).is_ok());
    }

    fn authorize_helper<'a>(
        registry: &'a OperationRegistry,
        id: &str,
        root: bool,
    ) -> Result<&'a Operation, RegistrationError> {
        registry.authorize(id, root)
    }

    #[test]
    fn invalid_id_rejected() {
        let op = Operation::new("bad id!", "Bad", "Bad ID", false, false);
        assert!(matches!(
            OperationRegistry::new().register(op),
            Err(RegistrationError::InvalidDefinition(_))
        ));
    }

    #[test]
    fn list_sorted() {
        let mut registry = OperationRegistry::new();
        registry
            .register(Operation::new("z.op", "Z", "Z", false, false))
            .unwrap();
        registry
            .register(Operation::new("a.op", "A", "A", false, false))
            .unwrap();
        let ids: Vec<_> = registry.list().iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, vec!["a.op", "z.op"]);
    }
}
