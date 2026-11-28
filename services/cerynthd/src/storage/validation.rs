//! Path and permission validation for worktree-to-volume mapping.
//!
//! Prevents path traversal, accidental mounting over protected directories,
//! and unauthorized cross-agent volume access.

use anyhow::{Context, Result, ensure};
use nix::sys::stat;
use nix::unistd::Uid;
use std::path::{Path, PathBuf};

/// Protected mount points that must never be mounted over.
const PROTECTED_PREFIXES: &[&str] = &[
    "/proc", "/sys", "/dev", "/run", "/boot", "/etc", "/var/lib", "/var/run", "/var/log", "/usr",
    "/bin", "/sbin", "/lib", "/lib64",
];

/// A permitted root directory for worktree discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermittedRoot {
    /// The root path.
    pub path: PathBuf,
    /// The UID that owns this root.
    pub owner_uid: u32,
}

/// Validates paths and permissions for volume operations.
#[derive(Debug, Clone)]
pub struct WorktreeValidator {
    permitted_roots: Vec<PermittedRoot>,
}

impl WorktreeValidator {
    /// Create a new validator with permitted roots owned by the given UID.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn new(roots: Vec<(PathBuf, u32)>) -> Result<Self> {
        ensure!(!roots.is_empty(), "at least one permitted root is required");
        let mut permitted_roots = Vec::new();
        for (path, owner_uid) in roots {
            ensure!(
                path.is_absolute(),
                "permitted root must be absolute: {}",
                path.display()
            );
            permitted_roots.push(PermittedRoot { path, owner_uid });
        }
        Ok(Self { permitted_roots })
    }

    /// Validate that a path is safe to use as a mount source or target.
    ///
    /// Checks for path traversal, protected prefixes, symlink escapes, and
    /// permitted-root containment.
    ///
    /// # Errors
    /// Returns an error if the path is unsafe or outside permitted roots.
    pub fn validate_path(&self, path: &Path) -> Result<PathBuf> {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("cannot canonicalize path: {}", path.display()))?;

        // Reject protected prefixes.
        let canonical_str = canonical.to_string_lossy();
        for prefix in PROTECTED_PREFIXES {
            if canonical_str == *prefix || canonical_str.starts_with(&format!("{prefix}/")) {
                anyhow::bail!(
                    "path is under protected prefix {}: {}",
                    prefix,
                    canonical.display()
                );
            }
        }

        // Verify the path is within a permitted root.
        let _root = self
            .permitted_roots
            .iter()
            .find(|root| canonical.starts_with(&root.path))
            .ok_or_else(|| {
                anyhow::anyhow!("path is outside permitted roots: {}", canonical.display())
            })?;

        Ok(canonical)
    }

    /// Check that a mount target path does not overlap with an existing volume mount.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn check_mount_conflict(&self, target: &Path, existing_mounts: &[PathBuf]) -> Result<()> {
        ensure!(target.is_absolute(), "mount target must be absolute");
        for existing in existing_mounts {
            ensure!(
                !target.starts_with(existing) && !existing.starts_with(target),
                "mount target overlaps an existing volume"
            );
        }
        Ok(())
    }

    /// Verify that the current UID owns the given path.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn check_ownership(&self, path: &Path) -> Result<()> {
        let metadata = nix::sys::stat::stat(path)
            .with_context(|| format!("cannot stat path: {}", path.display()))?;
        let current_uid = Uid::current().as_raw();
        ensure!(
            metadata.st_uid == current_uid,
            "path is not owned by current UID: {}",
            path.display()
        );
        Ok(())
    }

    /// Verify that a path has private permissions (owner-only access).
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn check_private_permissions(&self, path: &Path, is_directory: bool) -> Result<()> {
        let metadata =
            stat::lstat(path).with_context(|| format!("cannot lstat path: {}", path.display()))?;
        let expected = if is_directory { 0o700 } else { 0o600 };
        ensure!(
            metadata.st_mode & 0o7777 == expected,
            "path must have exact private permissions"
        );
        let kind = metadata.st_mode & libc::S_IFMT;
        ensure!(
            kind == if is_directory {
                libc::S_IFDIR
            } else {
                libc::S_IFREG
            },
            "path has wrong type or is a symlink"
        );
        ensure!(
            metadata.st_uid == Uid::current().as_raw(),
            "path must belong to current owner"
        );
        Ok(())
    }

    /// List the permitted roots.
    #[must_use]
    pub fn permitted_roots(&self) -> &[PermittedRoot] {
        &self.permitted_roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn exact_private_mode_and_mount_overlap_are_enforced() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir().join(format!("private-check-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let validator =
            WorktreeValidator::new(vec![(root.clone(), Uid::current().as_raw())]).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validator.check_private_permissions(&root, true).is_ok());
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(validator.check_private_permissions(&root, true).is_err());
        assert!(
            validator
                .check_mount_conflict(&root.join("child"), std::slice::from_ref(&root))
                .is_err()
        );
        std::fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn rejects_relative_path() {
        let validator =
            WorktreeValidator::new(vec![(temp_dir(), Uid::current().as_raw())]).unwrap();
        assert!(validator.validate_path(Path::new("relative")).is_err());
    }

    #[test]
    fn rejects_protected_prefix() {
        let validator =
            WorktreeValidator::new(vec![(temp_dir(), Uid::current().as_raw())]).unwrap();
        assert!(validator.validate_path(Path::new("/etc")).is_err());
        assert!(validator.validate_path(Path::new("/proc")).is_err());
    }

    #[test]
    fn rejects_outside_permitted_root() {
        let validator =
            WorktreeValidator::new(vec![(temp_dir(), Uid::current().as_raw())]).unwrap();
        assert!(validator.validate_path(Path::new("/")).is_err());
    }

    #[test]
    fn accepts_permitted_path() {
        let validator =
            WorktreeValidator::new(vec![(temp_dir(), Uid::current().as_raw())]).unwrap();
        let result = validator.validate_path(&temp_dir());
        assert!(result.is_ok());
    }

    #[test]
    fn requires_at_least_one_root() {
        assert!(WorktreeValidator::new(vec![]).is_err());
    }

    #[test]
    fn requires_absolute_root() {
        assert!(WorktreeValidator::new(vec![(PathBuf::from("relative"), 0)]).is_err());
    }
}
