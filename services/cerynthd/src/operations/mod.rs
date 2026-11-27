//! Operation registry, discovery, lifecycle, and resumable execution.
//!
//! Every supported operation is registered with a unique ID, permission
//! requirements, and resume capability. Execution state is persisted durably
//! so interrupted operations can be resumed without repeating destructive
//! actions.
//!
//! The types here form an authority API exercised directly by unit tests and
//! pending wiring into the daemon's request handlers.

#![allow(
    dead_code,
    unused_imports,
    clippy::unnested_or_patterns,
    clippy::trivially_copy_pass_by_ref,
    clippy::map_unwrap_or,
    clippy::cast_possible_truncation,
    clippy::doc_markdown,
    clippy::needless_pass_by_value
)]

pub mod discovery;
pub mod executor;
pub mod lifecycle;
pub mod persistence;
pub mod recovery;
pub mod registry;

pub use discovery::{DiscoveryResponse, OperationDescriptor};
pub use executor::{ExecutionHandler, Executor};
pub use lifecycle::{ExecutionRecord, LifecycleError, OperationState, StateTransition};
pub use registry::{OperationRegistry, RegistrationError};
