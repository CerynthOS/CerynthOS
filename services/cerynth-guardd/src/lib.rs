//! Cerynth Guard daemon library (Person 2).
//!
//! - [`config`]: validated configuration; builds the Guard state, failing closed
//! - [`state`]: sessions, delegation, decisions, approvals, audit, events
//! - [`dispatch`]: transport-independent handling and caller authorization
//! - [`git`]: brokered `git push` through a Guard-owned mirror
//! - [`vault`]: credential storage (directory or systemd-creds), redaction
//! - [`protocol`]: the request/response wire types
//!
//! Not in this crate yet: the egress gateway
//! integration. See `docs/security/HANDOFF.md`.

pub mod config;
pub mod dispatch;
pub mod git;
pub mod protocol;
pub mod state;
pub mod vault;

pub mod server;

pub mod client;

pub mod egress;
