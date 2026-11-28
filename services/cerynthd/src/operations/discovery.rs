//! Operation discovery interfaces for the harness.
//!
//! Exposes `operation.list`, `operation.describe`, and related discovery
//! operations backed by the central registry.

use serde::{Deserialize, Serialize};

use crate::operations::registry::{Operation, OperationRegistry, RegistrationError};

/// A lightweight descriptor returned by discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationDescriptor {
    pub id: String,
    pub name: String,
    pub description: String,
    pub requires_root: bool,
    pub resumable: bool,
}

impl From<&Operation> for OperationDescriptor {
    fn from(op: &Operation) -> Self {
        Self {
            id: op.id.clone(),
            name: op.name.clone(),
            description: op.description.clone(),
            requires_root: op.requires_root,
            resumable: op.resumable,
        }
    }
}

/// Response to an `operation.list` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryResponse {
    pub operations: Vec<OperationDescriptor>,
}

/// List all operations visible to the caller.
#[must_use]
pub fn list_operations(registry: &OperationRegistry) -> DiscoveryResponse {
    DiscoveryResponse {
        operations: registry
            .list()
            .into_iter()
            .map(OperationDescriptor::from)
            .collect(),
    }
}

/// Describe a single operation by ID.
///
/// # Errors
/// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
pub fn describe_operation(
    registry: &OperationRegistry,
    id: &str,
) -> Result<Operation, RegistrationError> {
    registry
        .get(id)
        .cloned()
        .ok_or_else(|| RegistrationError::UnknownOperation(id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_all_registered_operations() {
        let mut registry = OperationRegistry::new();
        registry
            .register(Operation::new("a.op", "A", "Desc A", false, true))
            .unwrap();
        registry
            .register(Operation::new("b.op", "B", "Desc B", true, false))
            .unwrap();
        let response = list_operations(&registry);
        assert_eq!(response.operations.len(), 2);
        assert_eq!(response.operations[0].id, "a.op");
        assert!(response.operations[0].resumable);
        assert!(response.operations[1].requires_root);
    }

    #[test]
    fn describes_known_operation() {
        let mut registry = OperationRegistry::new();
        registry
            .register(Operation::new("a.op", "A", "Desc A", false, true))
            .unwrap();
        let op = describe_operation(&registry, "a.op").unwrap();
        assert_eq!(op.name, "A");
        assert_eq!(op.description, "Desc A");
    }

    #[test]
    fn unknown_operation_rejected() {
        let registry = OperationRegistry::new();
        assert!(describe_operation(&registry, "nope").is_err());
    }
}
