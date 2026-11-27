//! Worktree-to-volume conversion and mapping for bounded agent storage.
//!
//! Provides safe conversion of an agent's working directory (worktree) into a
//! managed storage volume, with mount lifecycle, crash recovery, and
//! cross-agent isolation. Does not copy directory contents; it tracks
//! ownership, mount state, persistence, and cleanup.
//!
//! The types here form an owner API that is exercised directly by unit tests
//! and is pending wiring into the daemon's agent launch path.

#![allow(
    dead_code,
    unused_imports,
    clippy::unnecessary_wraps,
    clippy::unused_self,
    clippy::too_many_lines,
    clippy::map_unwrap_or
)]

pub mod mount;
pub mod validation;
pub mod volume;
pub mod worktree;

pub use mount::{MountManager, MountState};
pub use validation::{PermittedRoot, WorktreeValidator};
pub use volume::{VolumeManager, VolumeRecord};
pub use worktree::{DiscoveredWorktree, WorktreeDiscovery};
