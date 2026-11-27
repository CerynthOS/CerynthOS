//! Experimental owner-side platform APIs. No new privileged IPC is exposed.
//!
//! Callers own the execution boundary and authorization; agents must not be
//! given direct access to these administrative primitives.
pub mod operations;
pub mod resources;
pub mod storage;
