//! Cerynth Guard core library (Person 2).
//!
//! - [`capability`]: typed grants, deny-by-default matching, child ⊆ parent
//! - [`decision`]: `ActionRequest` / `SecurityDecision` / approvals (the contract)
//! - [`policy`]: Cedar guardrails combined with capabilities, failing closed
//! - [`profile`]: capabilities → bubblewrap + guard-exec command line
//! - [`confine`]: Landlock and seccomp, applied inside the sandbox
//! - [`audit`]: hash-chained, append-only audit log

pub mod audit;
pub mod capability;
pub mod confine;
pub mod decision;
pub mod policy;
pub mod profile;

pub use capability::{Capability, CapabilityKind, CapabilitySet, Mode};
pub use decision::{
    Action, ActionRequest, ApprovalAnswer, ApprovalInfo, ApprovalResponse, Enforcement, Reason,
    Risk, SecurityDecision, Verdict,
};
pub use profile::{GuardSession, HostPaths, SandboxProfile, SessionMode};
