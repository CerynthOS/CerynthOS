//! Volume management for worktree-to-volume conversion.
//!
//! Creates dedicated volumes for agents, assigns unique volume IDs, defines
//! ownership and permissions, and maintains metadata linking volumes to their
//! originating worktrees.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// The on-disk journal format for volume state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VolumeJournal {
    version: u32,
    capacity_bytes: u64,
    volumes: BTreeMap<Uuid, VolumeEntry>,
}

/// A single volume entry in the journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VolumeEntry {
    id: Uuid,
    worktree_path: PathBuf,
    resolved_worktree_path: PathBuf,
    image_bytes: u64,
    subtree_bytes_max: u64,
    status: VolumeStatus,
    owner_uid: u32,
    created_at_ms: u64,
    last_mounted_at_ms: Option<u64>,
    persistence_enabled: bool,
}

/// Status of a volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeStatus {
    Preparing,
    Ready,
    Mounted,
    Unmounting,
    Removing,
}

/// A volume record returned to callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeRecord {
    pub volume_id: Uuid,
    pub worktree_path: PathBuf,
    pub resolved_worktree_path: PathBuf,
    pub volume_directory: PathBuf,
    pub image_path: PathBuf,
    pub mount_path: PathBuf,
    pub image_bytes: u64,
    pub subtree_bytes_max: u64,
    pub status: VolumeStatus,
    pub owner_uid: u32,
    pub persistence_enabled: bool,
}

/// Manages volume creation, metadata, and lifecycle state.
#[derive(Debug)]
pub struct VolumeManager {
    /// Root directory where all volume state is stored.
    root: PathBuf,
    /// Total capacity available for all volumes.
    capacity_bytes: u64,
    _lock: nix::fcntl::Flock<std::fs::File>,
}

impl VolumeManager {
    /// Initialize a new volume authority at the given root with the given capacity.
    ///
    /// # Errors
    /// Returns an error if the root already contains a journal, or if capacity
    /// is below the minimum.
    pub fn init(root: &Path, capacity_bytes: u64) -> Result<Self> {
        ensure!(
            root.is_absolute(),
            "volume authority root must be absolute: {}",
            root.display()
        );
        ensure!(
            capacity_bytes >= 16 * 1024 * 1024,
            "capacity must be at least 16 MiB"
        );

        let journal_path = root.join("volumes.json");
        ensure!(
            !journal_path.exists(),
            "volume authority already initialized"
        );

        std::fs::create_dir_all(root).with_context(|| {
            format!("failed to create volume authority root: {}", root.display())
        })?;
        std::fs::set_permissions(root, Permissions::from_mode(0o700))
            .with_context(|| format!("failed to set permissions on {}", root.display()))?;

        let lock = lock_authority(root)?;
        ensure!(
            !journal_path.exists(),
            "volume authority already initialized"
        );
        let journal = VolumeJournal {
            version: 1,
            capacity_bytes,
            volumes: BTreeMap::new(),
        };
        store_journal_file(root, &journal)?;

        Ok(Self {
            root: root.to_path_buf(),
            capacity_bytes,
            _lock: lock,
        })
    }

    /// Load an existing volume authority from the given root.
    ///
    /// # Errors
    /// Returns an error if the journal is missing or invalid.
    pub fn load(root: &Path) -> Result<Self> {
        let lock = lock_authority(root)?;
        let journal_path = root.join("volumes.json");
        ensure!(
            journal_path.exists(),
            "volume authority not initialized at {}",
            root.display()
        );
        let data = std::fs::read(&journal_path).with_context(|| {
            format!("failed to read volume journal: {}", journal_path.display())
        })?;
        let journal: VolumeJournal = serde_json::from_slice(&data).with_context(|| {
            format!("failed to parse volume journal: {}", journal_path.display())
        })?;
        ensure!(journal.version == 1, "unsupported volume journal version");

        Ok(Self {
            root: root.to_path_buf(),
            capacity_bytes: journal.capacity_bytes,
            _lock: lock,
        })
    }

    /// Create a new volume linked to a discovered worktree.
    ///
    /// # Errors
    /// Returns an error if the volume ID is already in use, capacity would be
    /// exceeded, or the volume directory cannot be created.
    pub fn create_volume(
        &mut self,
        volume_id: Uuid,
        worktree: &crate::storage::worktree::DiscoveredWorktree,
        image_bytes: u64,
        subtree_bytes_max: u64,
        persistence_enabled: bool,
    ) -> Result<VolumeRecord> {
        ensure!(
            image_bytes >= 16 * 1024 * 1024,
            "image bytes must be at least 16 MiB"
        );
        ensure!(
            image_bytes <= subtree_bytes_max,
            "image bytes must not exceed subtree max"
        );
        ensure!(
            subtree_bytes_max <= self.capacity_bytes,
            "subtree max exceeds volume authority capacity"
        );
        ensure!(
            image_bytes.is_multiple_of(4096),
            "image bytes must be aligned to 4 KiB"
        );
        ensure!(
            subtree_bytes_max.is_multiple_of(4096),
            "subtree max must be aligned to 4 KiB"
        );

        let mut journal = self.read_journal()?;

        ensure!(
            !journal.volumes.contains_key(&volume_id),
            "volume ID already in use: {volume_id}"
        );

        // Check capacity against existing root volumes.
        let existing_root_usage: u64 = journal.volumes.values().map(|v| v.subtree_bytes_max).sum();
        ensure!(
            existing_root_usage
                .checked_add(subtree_bytes_max)
                .context("capacity overflow")?
                <= self.capacity_bytes,
            "volume authority capacity would be exceeded"
        );

        let volume_dir = self.root.join(volume_id.to_string());
        let mount_dir = volume_dir.join("mount");
        let image_path = volume_dir.join("workspace.ext4");

        // Create the volume directory with private permissions.
        std::fs::create_dir(&volume_dir).with_context(|| {
            format!(
                "failed to create volume directory: {}",
                volume_dir.display()
            )
        })?;
        std::fs::set_permissions(&volume_dir, Permissions::from_mode(0o700))
            .with_context(|| format!("failed to set permissions on {}", volume_dir.display()))?;

        // Create the mount point.
        std::fs::create_dir_all(&mount_dir).with_context(|| {
            format!("failed to create mount directory: {}", mount_dir.display())
        })?;
        std::fs::set_permissions(&mount_dir, Permissions::from_mode(0o700))
            .with_context(|| format!("failed to set permissions on {}", mount_dir.display()))?;

        // Create the backing image file (sparse, will be formatted on mount).
        let image_file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&image_path)
            .with_context(|| format!("failed to create volume image: {}", image_path.display()))?;
        image_file
            .set_len(image_bytes)
            .with_context(|| format!("failed to set image size: {}", image_path.display()))?;
        std::fs::set_permissions(&image_path, Permissions::from_mode(0o600))
            .with_context(|| format!("failed to set permissions on {}", image_path.display()))?;

        let now_ms = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("system time before UNIX epoch")?
                .as_millis(),
        )
        .context("timestamp overflow")?;
        let owner_uid = nix::unistd::Uid::current().as_raw();

        let entry = VolumeEntry {
            id: volume_id,
            worktree_path: worktree.supplied_path.clone(),
            resolved_worktree_path: worktree.resolved_path.clone(),
            image_bytes,
            subtree_bytes_max,
            status: VolumeStatus::Preparing,
            owner_uid,
            created_at_ms: now_ms,
            last_mounted_at_ms: None,
            persistence_enabled,
        };

        journal.volumes.insert(volume_id, entry);
        store_journal_file(self.root.as_path(), &journal)?;

        Ok(VolumeRecord {
            volume_id,
            worktree_path: worktree.supplied_path.clone(),
            resolved_worktree_path: worktree.resolved_path.clone(),
            volume_directory: volume_dir,
            image_path,
            mount_path: mount_dir,
            image_bytes,
            subtree_bytes_max,
            status: VolumeStatus::Preparing,
            owner_uid,
            persistence_enabled,
        })
    }

    /// Mark a volume as ready for mounting.
    ///
    /// # Errors
    /// Returns an error if the volume is unknown or not in Preparing state.
    pub fn mark_ready(&mut self, volume_id: Uuid) -> Result<VolumeRecord> {
        let mut journal = self.read_journal()?;
        let entry = journal
            .volumes
            .get_mut(&volume_id)
            .ok_or_else(|| anyhow::anyhow!("unknown volume: {volume_id}"))?;
        ensure!(
            entry.status == VolumeStatus::Preparing,
            "volume {volume_id} is not in Preparing state"
        );
        entry.status = VolumeStatus::Ready;
        let entry_clone = entry.clone();
        store_journal_file(self.root.as_path(), &journal)?;
        self.entry_to_record(&entry_clone, volume_id)
    }

    /// Mark a volume as mounted.
    ///
    /// # Errors
    /// Returns an error if the volume is unknown or not in Ready state.
    pub fn mark_mounted(&mut self, volume_id: Uuid) -> Result<VolumeRecord> {
        let mut journal = self.read_journal()?;
        let entry = journal
            .volumes
            .get_mut(&volume_id)
            .ok_or_else(|| anyhow::anyhow!("unknown volume: {volume_id}"))?;
        ensure!(
            entry.status == VolumeStatus::Ready,
            "volume {volume_id} is not in Ready state"
        );
        let now_ms = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("system time before UNIX epoch")?
                .as_millis(),
        )
        .context("timestamp overflow")?;
        entry.status = VolumeStatus::Mounted;
        entry.last_mounted_at_ms = Some(now_ms);
        let entry_clone = entry.clone();
        store_journal_file(self.root.as_path(), &journal)?;
        self.entry_to_record(&entry_clone, volume_id)
    }

    /// Mark a volume as unmounted.
    ///
    /// # Errors
    /// Returns an error if the volume is unknown or not in Mounted state.
    pub fn mark_unmounted(&mut self, volume_id: Uuid) -> Result<VolumeRecord> {
        let mut journal = self.read_journal()?;
        let entry = journal
            .volumes
            .get_mut(&volume_id)
            .ok_or_else(|| anyhow::anyhow!("unknown volume: {volume_id}"))?;
        ensure!(
            entry.status == VolumeStatus::Mounted,
            "volume {volume_id} is not in Mounted state"
        );
        entry.status = VolumeStatus::Ready;
        let entry_clone = entry.clone();
        store_journal_file(self.root.as_path(), &journal)?;
        self.entry_to_record(&entry_clone, volume_id)
    }

    /// Mark a volume for removal.
    ///
    /// # Errors
    /// Returns an error if the volume is unknown or currently mounted.
    pub fn mark_removing(&mut self, volume_id: Uuid) -> Result<VolumeRecord> {
        let mut journal = self.read_journal()?;
        let entry = journal
            .volumes
            .get_mut(&volume_id)
            .ok_or_else(|| anyhow::anyhow!("unknown volume: {volume_id}"))?;
        ensure!(
            entry.status != VolumeStatus::Mounted,
            "volume {volume_id} is mounted; unmount before removal"
        );
        entry.status = VolumeStatus::Removing;
        let entry_clone = entry.clone();
        store_journal_file(self.root.as_path(), &journal)?;
        self.entry_to_record(&entry_clone, volume_id)
    }

    /// Remove a volume entry and its directory from the journal and disk.
    ///
    /// # Errors
    /// Returns an error if the volume is unknown, mounted, or has children.
    pub fn remove_volume(&mut self, volume_id: Uuid) -> Result<VolumeRecord> {
        let mut journal = self.read_journal()?;
        let entry = journal
            .volumes
            .get(&volume_id)
            .ok_or_else(|| anyhow::anyhow!("unknown volume: {volume_id}"))?;
        ensure!(
            entry.status != VolumeStatus::Mounted,
            "volume {volume_id} is mounted; unmount before removal"
        );
        ensure!(
            !journal
                .volumes
                .values()
                .any(|v| v.id != volume_id && v.worktree_path.starts_with(&entry.worktree_path)),
            "volume {volume_id} has dependent worktrees; remove them first"
        );

        // Clone all needed data before mutably borrowing the journal.
        let entry_clone = entry.clone();

        let volume_dir = self.root.join(volume_id.to_string());
        let image_path = volume_dir.join("workspace.ext4");
        let mount_path = volume_dir.join("mount");

        ensure!(
            !crate::storage::mount::MountManager::is_mounted(&mount_path),
            "kernel mount still active; unmount before removal"
        );
        // Remove backing image.
        if image_path.exists() {
            std::fs::remove_file(&image_path).with_context(|| {
                format!("failed to remove volume image: {}", image_path.display())
            })?;
        }
        // Remove mount directory.
        if mount_path.exists() {
            std::fs::remove_dir(&mount_path)?;
        }
        // Remove volume directory.
        if volume_dir.exists() {
            std::fs::remove_dir(&volume_dir)?;
        }

        journal.volumes.remove(&volume_id);
        store_journal_file(self.root.as_path(), &journal)?;

        Ok(VolumeRecord {
            volume_id,
            worktree_path: entry_clone.worktree_path.clone(),
            resolved_worktree_path: entry_clone.resolved_worktree_path.clone(),
            volume_directory: volume_dir,
            image_path,
            mount_path,
            image_bytes: entry_clone.image_bytes,
            subtree_bytes_max: entry_clone.subtree_bytes_max,
            status: VolumeStatus::Removing,
            owner_uid: entry_clone.owner_uid,
            persistence_enabled: entry_clone.persistence_enabled,
        })
    }

    /// Get a volume record by ID.
    ///
    /// # Errors
    /// Returns an error if the volume is unknown.
    pub fn get_volume(&self, volume_id: Uuid) -> Result<VolumeRecord> {
        let journal = self.read_journal()?;
        let entry = journal
            .volumes
            .get(&volume_id)
            .ok_or_else(|| anyhow::anyhow!("unknown volume: {volume_id}"))?;
        self.entry_to_record(entry, volume_id)
    }

    /// List all volumes.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn list_volumes(&self) -> Result<Vec<VolumeRecord>> {
        let journal = self.read_journal()?;
        let mut records = Vec::new();
        for (id, entry) in &journal.volumes {
            records.push(self.entry_to_record(entry, *id)?);
        }
        Ok(records)
    }

    /// Find a volume by its worktree path.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn find_by_worktree(&self, worktree_path: &Path) -> Result<Option<VolumeRecord>> {
        let journal = self.read_journal()?;
        for (id, entry) in &journal.volumes {
            if entry.worktree_path == worktree_path || entry.resolved_worktree_path == worktree_path
            {
                return Ok(Some(self.entry_to_record(entry, *id)?));
            }
        }
        Ok(None)
    }

    /// Check whether a volume exists and is mounted.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn is_mounted(&self, volume_id: Uuid) -> Result<bool> {
        let journal = self.read_journal()?;
        let entry = journal
            .volumes
            .get(&volume_id)
            .ok_or_else(|| anyhow::anyhow!("unknown volume: {volume_id}"))?;
        Ok(entry.status == VolumeStatus::Mounted)
    }

    /// Recover stale mounts after a crash: reset any Mounted volumes to Ready.
    ///
    /// Returns the list of volume IDs that were recovered.
    ///
    /// # Errors
    /// Returns an error when validation, state lookup or underlying I/O prevents the requested operation.
    pub fn recover_stale_mounts(&mut self) -> Result<Vec<Uuid>> {
        let mut journal = self.read_journal()?;
        let mut recovered = Vec::new();
        for entry in journal.volumes.values_mut() {
            if entry.status == VolumeStatus::Mounted {
                let mount = self.root.join(entry.id.to_string()).join("mount");
                let mounted = crate::storage::mount::MountManager::is_mounted(&mount);
                if mounted && std::fs::read_dir(&mount).is_ok() {
                    continue;
                }
                if mounted {
                    crate::storage::mount::MountManager::new(&self.root)
                        .unmount_volume(&entry.id)?;
                }
                entry.status = VolumeStatus::Ready;
                entry.last_mounted_at_ms = None;
                recovered.push(entry.id);
            }
        }
        if !recovered.is_empty() {
            store_journal_file(self.root.as_path(), &journal)?;
        }
        Ok(recovered)
    }

    /// Get the volume authority root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Get the total capacity.
    #[must_use]
    pub fn capacity_bytes(&self) -> u64 {
        self.capacity_bytes
    }

    fn read_journal(&self) -> Result<VolumeJournal> {
        let journal_path = self.root.join("volumes.json");
        let data = std::fs::read(&journal_path).with_context(|| {
            format!("failed to read volume journal: {}", journal_path.display())
        })?;
        serde_json::from_slice(&data)
            .with_context(|| format!("failed to parse volume journal: {}", journal_path.display()))
    }

    fn entry_to_record(&self, entry: &VolumeEntry, volume_id: Uuid) -> Result<VolumeRecord> {
        let volume_dir = self.root.join(volume_id.to_string());
        Ok(VolumeRecord {
            volume_id,
            worktree_path: entry.worktree_path.clone(),
            resolved_worktree_path: entry.resolved_worktree_path.clone(),
            volume_directory: volume_dir.clone(),
            image_path: volume_dir.join("workspace.ext4"),
            mount_path: volume_dir.join("mount"),
            image_bytes: entry.image_bytes,
            subtree_bytes_max: entry.subtree_bytes_max,
            status: entry.status,
            owner_uid: entry.owner_uid,
            persistence_enabled: entry.persistence_enabled,
        })
    }
}

fn lock_authority(root: &Path) -> Result<nix::fcntl::Flock<std::fs::File>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let meta = std::fs::symlink_metadata(root)?;
    ensure!(
        root.is_absolute()
            && root.canonicalize()? == root
            && meta.is_dir()
            && meta.uid() == nix::unistd::Uid::current().as_raw()
            && meta.permissions().mode() & 0o7777 == 0o700,
        "volume authority must be absolute, private, owned and non-symlink"
    );
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join("volumes.lock"))?;
    nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
        .map_err(|(_, error)| anyhow::anyhow!("volume authority already owned: {error}"))
}

fn store_journal_file(root: &Path, journal: &VolumeJournal) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let journal_path = root.join("volumes.json");
    let tmp_path = root.join(format!("volumes-{}.tmp", Uuid::new_v4()));
    let data = serde_json::to_vec_pretty(journal)?;
    let result = (|| -> Result<()> {
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp_path)?;
        output.write_all(&data)?;
        output.sync_all()?;
        std::fs::rename(&tmp_path, &journal_path)?;
        std::fs::File::open(root)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn temp_worktree() -> crate::storage::worktree::DiscoveredWorktree {
        crate::storage::worktree::DiscoveredWorktree {
            supplied_path: temp_dir().join("test-worktree"),
            resolved_path: temp_dir().join("test-worktree"),
            permitted_root: temp_dir(),
            observed_at: 0,
            is_git_worktree: false,
        }
    }

    #[test]
    fn init_and_load_volume_manager() {
        let root = temp_dir().join(format!("vol-test-{}", Uuid::new_v4()));
        let _ = std::fs::remove_dir_all(&root);
        let mgr = VolumeManager::init(&root, 1024 * 1024 * 1024).unwrap();
        assert_eq!(mgr.capacity_bytes(), 1024 * 1024 * 1024);

        assert!(
            VolumeManager::load(&root).is_err(),
            "second live authority must be denied"
        );
        drop(mgr);
        let loaded = VolumeManager::load(&root).unwrap();
        assert_eq!(loaded.capacity_bytes(), 1024 * 1024 * 1024);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn init_rejects_existing_journal() {
        let root = temp_dir().join(format!("vol-test-{}", Uuid::new_v4()));
        let _ = std::fs::remove_dir_all(&root);
        VolumeManager::init(&root, 1024 * 1024 * 1024).unwrap();
        assert!(VolumeManager::init(&root, 1024 * 1024 * 1024).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn create_and_mark_volume() {
        let root = temp_dir().join(format!("vol-test-{}", Uuid::new_v4()));
        let _ = std::fs::remove_dir_all(&root);
        let mut mgr = VolumeManager::init(&root, 1024 * 1024 * 1024).unwrap();

        let wt = temp_worktree();
        let volume_id = Uuid::new_v4();
        let record = mgr
            .create_volume(volume_id, &wt, 32 * 1024 * 1024, 64 * 1024 * 1024, true)
            .unwrap();
        assert_eq!(record.volume_id, volume_id);
        assert_eq!(record.status, VolumeStatus::Preparing);

        let ready = mgr.mark_ready(volume_id).unwrap();
        assert_eq!(ready.status, VolumeStatus::Ready);

        let mounted = mgr.mark_mounted(volume_id).unwrap();
        assert_eq!(mounted.status, VolumeStatus::Mounted);

        let unmounted = mgr.mark_unmounted(volume_id).unwrap();
        assert_eq!(unmounted.status, VolumeStatus::Ready);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recover_stale_mounts() {
        let root = temp_dir().join(format!("vol-test-{}", Uuid::new_v4()));
        let _ = std::fs::remove_dir_all(&root);
        let mut mgr = VolumeManager::init(&root, 1024 * 1024 * 1024).unwrap();

        let wt = temp_worktree();
        let volume_id = Uuid::new_v4();
        let _ = mgr
            .create_volume(volume_id, &wt, 32 * 1024 * 1024, 64 * 1024 * 1024, true)
            .unwrap();
        let _ = mgr.mark_ready(volume_id).unwrap();
        let _ = mgr.mark_mounted(volume_id).unwrap();

        // Simulate crash: volume is marked Mounted but process is gone.
        let recovered = mgr.recover_stale_mounts().unwrap();
        assert_eq!(recovered, vec![volume_id]);
        assert!(!mgr.is_mounted(volume_id).unwrap());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_by_worktree() {
        let root = temp_dir().join(format!("vol-test-{}", Uuid::new_v4()));
        let _ = std::fs::remove_dir_all(&root);
        let mut mgr = VolumeManager::init(&root, 1024 * 1024 * 1024).unwrap();

        let wt = temp_worktree();
        let volume_id = Uuid::new_v4();
        let _ = mgr
            .create_volume(volume_id, &wt, 32 * 1024 * 1024, 64 * 1024 * 1024, true)
            .unwrap();

        let found = mgr.find_by_worktree(&wt.supplied_path).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().volume_id, volume_id);

        let not_found = mgr
            .find_by_worktree(std::path::Path::new("/nonexistent"))
            .unwrap();
        assert!(not_found.is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_unaligned_image_size() {
        let root = temp_dir().join(format!("vol-test-{}", Uuid::new_v4()));
        let _ = std::fs::remove_dir_all(&root);
        let mut mgr = VolumeManager::init(&root, 1024 * 1024 * 1024).unwrap();

        let wt = temp_worktree();
        let volume_id = Uuid::new_v4();
        let result = mgr.create_volume(volume_id, &wt, 1000, 64 * 1024 * 1024, true);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
