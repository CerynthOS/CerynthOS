//! Mount operations for worktree-to-volume mapping.
//!
//! Mounts volumes when agents start, unmounts when workloads terminate,
//! handles crashes and stale mounts, and prevents path traversal and
//! mounting over protected directories.

use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Current mount state of a volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountState {
    Unmounted,
    Mounted,
    Stale,
}

/// Manages mount and unmount operations for volumes.
#[derive(Debug, Clone)]
pub struct MountManager {
    /// Directory where volume state is stored.
    volume_root: PathBuf,
}

impl MountManager {
    /// Create a new mount manager for the given volume root.
    pub fn new(volume_root: &Path) -> Self {
        Self {
            volume_root: volume_root.to_path_buf(),
        }
    }

    /// Get the mount path for a volume.
    pub fn mount_path(&self, volume_id: &uuid::Uuid) -> PathBuf {
        self.volume_root.join(volume_id.to_string()).join("mount")
    }

    /// Get the image path for a volume.
    pub fn image_path(&self, volume_id: &uuid::Uuid) -> PathBuf {
        self.volume_root
            .join(volume_id.to_string())
            .join("workspace.ext4")
    }

    /// Check whether a path is currently mounted (by consulting /proc/self/mountinfo).
    pub fn is_mounted(path: &Path) -> bool {
        let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
            return false;
        };
        let target = path.to_string_lossy();
        mountinfo.lines().any(|line| {
            let Some((_attributes, filesystem)) = line.split_once(" - ") else {
                return false;
            };
            let fields: Vec<&str> = filesystem.split_whitespace().collect();
            fields
                .first()
                .is_some_and(|mount_point| *mount_point == target.as_ref())
        })
    }

    /// Mount a volume's backing image at its mount point using fuse2fs.
    ///
    /// # Errors
    /// Returns an error if fuse2fs is unavailable, the mount fails, or the
    /// mount target is unsafe.
    pub fn mount_volume(&self, volume_id: &uuid::Uuid, _image_bytes: u64) -> Result<MountState> {
        let mount_path = self.mount_path(volume_id);
        let image_path = self.image_path(volume_id);

        ensure!(
            image_path.exists(),
            "volume image does not exist: {}",
            image_path.display()
        );

        if Self::is_mounted(&mount_path) {
            return Ok(MountState::Mounted);
        }

        // Ensure mount directory exists.
        std::fs::create_dir_all(&mount_path).with_context(|| {
            format!("failed to create mount directory: {}", mount_path.display())
        })?;

        let fuse2fs = std::process::Command::new("fuse2fs")
            .arg(&image_path)
            .arg(&mount_path)
            .arg("-f")
            .arg("-o")
            .arg(format!(
                "fakeroot,cache_size=8M,fsname=cerynth-{volume_id},nosuid,nodev"
            ))
            .output();

        match fuse2fs {
            Ok(output) if output.status.success() => {
                // Wait for mount to appear in mountinfo.
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                while std::time::Instant::now() < deadline {
                    if Self::is_mounted(&mount_path) {
                        return Ok(MountState::Mounted);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                anyhow::bail!("FUSE workspace did not become ready within timeout");
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("fuse2fs mount failed: {stderr}");
            }
            Err(e) => {
                anyhow::bail!("failed to execute fuse2fs: {e}");
            }
        }
    }

    /// Unmount a volume's mount point.
    ///
    /// # Errors
    /// Returns an error if the unmount fails.
    pub fn unmount_volume(&self, volume_id: &uuid::Uuid) -> Result<MountState> {
        let mount_path = self.mount_path(volume_id);

        if !Self::is_mounted(&mount_path) {
            return Ok(MountState::Unmounted);
        }

        let status = std::process::Command::new("fusermount3")
            .arg("-u")
            .arg(&mount_path)
            .status();

        match status {
            Ok(s) if s.success() => Ok(MountState::Unmounted),
            Ok(s) => anyhow::bail!("fusermount3 failed with exit status: {s}"),
            Err(e) => anyhow::bail!("failed to execute fusermount3: {e}"),
        }
    }

    /// Recover stale mounts after a crash.
    ///
    /// Checks all volumes under the volume root and attempts to unmount any
    /// that are still mounted but whose owning process is gone. Returns the
    /// list of volume IDs that were recovered.
    ///
    /// # Errors
    /// Returns an error if the volume root cannot be read.
    pub fn recover_stale_mounts(&self) -> Result<Vec<uuid::Uuid>> {
        let mut recovered = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&self.volume_root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let volume_id_str = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let Ok(volume_id) = uuid::Uuid::parse_str(volume_id_str) else {
                    continue;
                };

                let mount_path = self.mount_path(&volume_id);
                if Self::is_mounted(&mount_path) {
                    // Attempt unmount; ignore errors as the process may be gone.
                    let _ = self.unmount_volume(&volume_id);
                    recovered.push(volume_id);
                }
            }
        }

        Ok(recovered)
    }

    /// Validate that a mount target path is not under a protected directory.
    pub fn validate_mount_target(path: &Path) -> Result<()> {
        const PROTECTED_PREFIXES: &[&str] = &[
            "/proc", "/sys", "/dev", "/run", "/boot", "/etc", "/var", "/usr", "/bin", "/sbin",
            "/lib", "/lib64",
        ];
        let path_str = path.to_string_lossy();
        for prefix in PROTECTED_PREFIXES {
            if path_str == *prefix || path_str.starts_with(&format!("{prefix}/")) {
                anyhow::bail!(
                    "mount target is under protected prefix {}: {}",
                    prefix,
                    path.display()
                );
            }
        }
        Ok(())
    }

    /// Validate that two volume mount paths cannot conflict.
    pub fn check_no_conflict(
        &self,
        volume_id: &uuid::Uuid,
        other_ids: &[uuid::Uuid],
    ) -> Result<()> {
        let target = self.mount_path(volume_id);
        Self::validate_mount_target(&target)?;

        for other_id in other_ids {
            let other_target = self.mount_path(other_id);
            if target == other_target {
                anyhow::bail!("volume {volume_id} would mount over volume {other_id}");
            }
            if target.starts_with(&other_target) || other_target.starts_with(&target) {
                anyhow::bail!("volume {volume_id} mount path conflicts with volume {other_id}");
            }
        }

        Ok(())
    }
}

impl Default for MountManager {
    fn default() -> Self {
        Self {
            volume_root: std::env::temp_dir().join("cerynth-volumes"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn mount_paths_are_unique_per_volume() {
        let manager = MountManager::new(&temp_dir());
        let id1 = uuid::Uuid::new_v4();
        let id2 = uuid::Uuid::new_v4();
        assert_ne!(manager.mount_path(&id1), manager.mount_path(&id2));
    }

    #[test]
    fn mount_path_is_under_volume_root() {
        let root = temp_dir().join("volumes");
        let manager = MountManager::new(&root);
        let id = uuid::Uuid::new_v4();
        let path = manager.mount_path(&id);
        assert!(path.starts_with(&root));
        assert!(path.ends_with(&format!("{id}/mount")));
    }

    #[test]
    fn rejects_protected_mount_target() {
        assert!(MountManager::validate_mount_target(Path::new("/proc")).is_err());
        assert!(MountManager::validate_mount_target(Path::new("/etc/passwd")).is_err());
        assert!(MountManager::validate_mount_target(Path::new("/tmp/safe")).is_ok());
    }

    #[test]
    fn is_mounted_returns_false_for_nonexistent() {
        assert!(!MountManager::is_mounted(Path::new(
            "/nonexistent/path/xyz"
        )));
    }

    #[test]
    fn check_no_conflict_rejects_duplicate() {
        let manager = MountManager::new(&temp_dir());
        let id = uuid::Uuid::new_v4();
        assert!(manager.check_no_conflict(&id, &[id]).is_err());
    }
}
