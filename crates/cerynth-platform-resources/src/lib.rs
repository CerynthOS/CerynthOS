//! cgroup v2 session resource control for `CerynthOS`.
//!
//! This crate is the *mechanism* half of the resource contract. Person 3
//! decides what a session should get; this crate applies that decision to a
//! cgroup v2 subtree, reports what was actually enforced, and reads usage and
//! pressure back out.
//!
//! It deliberately does not touch `cerynth-scx`. The scheduler is only worth
//! changing once cgroups have been measured and shown to be insufficient.
//!
//! The wire contract is documented in
//! `docs/contracts/resource-budget-v1.md`.

// The cgroup layer attaches `anyhow` context at every fallible call site, so
// per-function `# Errors` sections would repeat the same I/O caveat.
#![allow(clippy::missing_errors_doc)]

pub mod budget;
pub mod cgroup;
pub mod stats;
pub mod support;

pub use budget::{
    AppliedLimit, AppliedLimits, BudgetError, BudgetStatus, CpuMax, DEFAULT_CPU_PERIOD_US,
    Enforcement, PriorityClass, RESOURCE_BUDGET_SCHEMA_VERSION, ResourceBudget, ResourceLimits,
};
pub use cgroup::{CgroupRoot, ControllerEnable, SESSION_PREFIX, SessionCgroup};
pub use stats::{
    CpuUsage, IoDeviceUsage, MemoryEvents, MemoryUsage, PidsUsage, Pressure, PressureLine,
    PressureValues, RESOURCE_USAGE_SCHEMA_VERSION, ResourceUsage,
};
pub use support::{
    LimitSupport, PsiSupport, SUPPORT_MATRIX_SCHEMA_VERSION, SupportMatrix, SystemdSupport,
};
