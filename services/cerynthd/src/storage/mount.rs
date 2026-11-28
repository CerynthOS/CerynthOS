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
    #[must_use]
    pub fn new(volume_root: &Path) -> Self {
        Self {
            volume_root: volume_root.to_path_buf(),
        }
    }

    /// Get the mount path for a volume.
    #[must_use]
    pub fn mount_path(&self, volume_id: &uuid::Uuid) -> PathBuf {
        self.volume_root.join(volume_id.to_string()).join("mount")
    }

    /// Get the image path for a volume.
    #[must_use]
    pub fn image_path(&self, volume_id: &uuid::Uuid) -> PathBuf {
        self.volume_root
            .join(volume_id.to_string())
            .join("workspace.ext4")
    }

    /// Check whether a path is currently mounted (by consulting /proc/self/mountinfo).
    #[must_use]
    pub fn is_mounted(path: &Path) -> bool {
        let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
            return false;
        };
        mountinfo.lines().any(|line| {
            let Some((attributes, _)) = line.split_once(" - ") else {
                return false;
            };
            attributes
                .split_whitespace()
                .nth(4)
                .is_some_and(|field| decode_mount_path(field).as_deref() == Some(path))
        })
    }

    /// The owned helper service name, stable across owner-process restarts.
    #[must_use]
    pub fn helper_unit(&self, volume_id: &uuid::Uuid) -> String {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.volume_root.hash(&mut hash);
        format!(
            "cerynth-rust-volume-{:016x}-{volume_id}.service",
            hash.finish()
        )
    }

    fn validate_owned_paths(&self, volume_id: &uuid::Uuid, image_bytes: u64) -> Result<()> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            self.volume_root.is_absolute(),
            "volume authority must be absolute"
        );
        ensure!(
            self.volume_root.canonicalize()? == self.volume_root,
            "volume authority contains symlinks"
        );
        let directory = self.volume_root.join(volume_id.to_string());
        for path in [&self.volume_root, &directory] {
            let meta = std::fs::symlink_metadata(path)?;
            ensure!(
                meta.is_dir()
                    && meta.uid() == nix::unistd::Uid::current().as_raw()
                    && meta.permissions().mode() & 0o7777 == 0o700,
                "volume directory must be private, owned and non-symlink"
            );
        }
        if !Self::is_mounted(&self.mount_path(volume_id)) {
            let meta = std::fs::symlink_metadata(self.mount_path(volume_id))?;
            ensure!(
                meta.is_dir()
                    && meta.uid() == nix::unistd::Uid::current().as_raw()
                    && meta.permissions().mode() & 0o7777 == 0o700,
                "unmounted workspace must be a private owned directory"
            );
        }
        let image = std::fs::symlink_metadata(self.image_path(volume_id))?;
        ensure!(
            image.is_file()
                && image.uid() == nix::unistd::Uid::current().as_raw()
                && image.permissions().mode() & 0o7777 == 0o600
                && image.len() == image_bytes,
            "volume image identity, permission or capacity mismatch"
        );
        Ok(())
    }

    /// Mount only a private owned image through a bounded user-systemd service.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn mount_volume(&self, volume_id: &uuid::Uuid, image_bytes: u64) -> Result<MountState> {
        let mount_path = self.mount_path(volume_id);
        self.validate_owned_paths(volume_id, image_bytes)?;
        if Self::is_mounted(&mount_path) {
            return Ok(MountState::Mounted);
        }
        let binary = std::env::var_os("CERYNTH_FUSE2FS_BINARY")
            .map_or_else(|| PathBuf::from("/usr/bin/fuse2fs"), PathBuf::from);
        ensure!(
            binary.is_absolute() && binary.is_file(),
            "select an absolute installed fuse2fs executable"
        );
        let mut command = std::process::Command::new("/usr/bin/systemd-run");
        command
            .args(["--user", "--quiet", "--collect", "--service-type=exec"])
            .arg(format!("--unit={}", self.helper_unit(volume_id)))
            .args([
                "--property=MemoryMax=134217728",
                "--property=TasksMax=32",
                "--property=CPUWeight=50",
                "--property=KillMode=control-group",
            ]);
        if let Some(path) = std::env::var_os("CERYNTH_FUSE_LIBRARY_PATH") {
            command.arg(format!(
                "--setenv=LD_LIBRARY_PATH={}",
                path.to_string_lossy()
            ));
        }
        let output = command
            .arg("--")
            .arg(binary)
            .arg(self.image_path(volume_id))
            .arg(&mount_path)
            .args(["-f", "-o"])
            .arg(format!(
                "fakeroot,cache_size=8M,fsname=cerynth-{volume_id},nosuid,nodev"
            ))
            .output()
            .context("start owned FUSE service")?;
        ensure!(
            output.status.success(),
            "start FUSE service: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if Self::is_mounted(&mount_path) {
                return Ok(MountState::Mounted);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        self.stop_helper(volume_id)?;
        anyhow::bail!("FUSE workspace readiness timed out")
    }

    fn stop_helper(&self, volume_id: &uuid::Uuid) -> Result<()> {
        let unit = self.helper_unit(volume_id);
        let state = std::process::Command::new("/usr/bin/systemctl")
            .args(["--user", "show", &unit, "--property=LoadState", "--value"])
            .output()?;
        if String::from_utf8_lossy(&state.stdout).trim() == "not-found" {
            return Ok(());
        }
        let result = std::process::Command::new("/usr/bin/systemctl")
            .args(["--user", "stop", &unit])
            .output()?;
        if !result.status.success() {
            let after = std::process::Command::new("/usr/bin/systemctl")
                .args(["--user", "show", &unit, "--property=LoadState", "--value"])
                .output()?;
            ensure!(
                String::from_utf8_lossy(&after.stdout).trim() == "not-found",
                "could not stop owned FUSE helper"
            );
        }
        Ok(())
    }

    /// Unmount a volume's mount point.
    ///
    /// # Errors
    /// Returns an error if the unmount fails.
    pub fn unmount_volume(&self, volume_id: &uuid::Uuid) -> Result<MountState> {
        let mount_path = self.mount_path(volume_id);

        if Self::is_mounted(&mount_path) {
            let output = std::process::Command::new("/usr/bin/fusermount3")
                .args(["-u", "-z"])
                .arg(&mount_path)
                .output()?;
            ensure!(
                output.status.success(),
                "unmount failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        self.stop_helper(volume_id)?;
        ensure!(
            !Self::is_mounted(&mount_path),
            "FUSE mount still present after cleanup"
        );
        Ok(MountState::Unmounted)
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
                    // Do not tear down healthy mounts merely because a new manager starts.
                    if std::fs::read_dir(&mount_path).is_err() {
                        self.unmount_volume(&volume_id)?;
                        recovered.push(volume_id);
                    }
                }
            }
        }

        Ok(recovered)
    }

    /// Validate that a mount target path is not under a protected directory.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
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
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
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

fn decode_mount_path(field: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let input = field.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'\\' {
            if index + 3 >= input.len()
                || !input[index + 1..index + 4]
                    .iter()
                    .all(|b| (b'0'..=b'7').contains(b))
            {
                return None;
            }
            let value = (u16::from(input[index + 1] - b'0') * 64)
                + (u16::from(input[index + 2] - b'0') * 8)
                + u16::from(input[index + 3] - b'0');
            decoded.push(u8::try_from(value).ok()?);
            index += 4;
        } else {
            decoded.push(input[index]);
            index += 1;
        }
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
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
    fn actual_root_mount_and_escaped_unicode_path_are_parsed() {
        assert!(MountManager::is_mounted(Path::new("/")));
        assert_eq!(
            decode_mount_path("/tmp/α\\040volume"),
            Some(PathBuf::from("/tmp/α volume"))
        );
        assert!(decode_mount_path("/tmp/invalid\\999").is_none());
    }

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
        assert!(path.ends_with(format!("{id}/mount")));
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
