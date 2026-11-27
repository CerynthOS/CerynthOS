//! Machine-wide resource registration, accounting, and enforcement.
//!
//! This module provides a central authority for agent resource identities and
//! reads actual consumption from kernel-maintained cgroup v2 interfaces. It
//! does not trust agent self-reports. Enforcement uses cgroups; scheduler
//! policy remains a separate concern.
//!
//! The types here form an authority API exercised directly by unit tests and
//! pending integration into the daemon's request handlers.

#![allow(
    dead_code,
    unused_imports,
    clippy::unnecessary_lazy_evaluations,
    clippy::struct_field_names,
    clippy::cast_possible_wrap
)]

pub mod accounting;
pub mod cpu;
pub mod enforcement;
pub mod memory;
pub mod process;
pub mod registration;

pub use accounting::{AccountingService, MachineUsage};
pub use registration::{AgentRegistration, ExecutionState, RegistrationError, RegistrationStore};
