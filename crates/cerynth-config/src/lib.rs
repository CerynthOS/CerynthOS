//! Configuration and runtime state management for CerynthOS.

pub mod config;
pub mod error;
pub mod state;

// Re-export the public API.
pub use config::Config;
pub use error::ConfigError;
pub use state::RuntimeState;
