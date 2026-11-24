//! Verify a private owner volume journal and the actual kernel mount before launch.
use anyhow::{Context, Result, ensure};
use cerynth_agent_api::WorkspaceVolume;
use fs2::FileExt;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    os::unix::{
        ffi::OsStringExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    capacity_bytes: u64,
    volumes: BTreeMap<Uuid, Volume>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Volume {
    id: Uuid,
    parent: Option<Uuid>,
    image_bytes: u64,
    subtree_bytes_max: u64,
    seed: Option<String>,
    status: String,
}

fn private(path: &Path, directory: bool) -> Result<()> {
    let info = std::fs::symlink_metadata(path)?;
    ensure!(
        (if directory {
            info.is_dir()
        } else {
            info.is_file()
        }) && info.uid() == nix::unistd::getuid().as_raw()
            && info.mode() & 0o777 == if directory { 0o700 } else { 0o600 },
        "workspace authority path is not private and owned"
    );
    Ok(())
}

fn unescape(text: &str) -> PathBuf {
    let mut result = Vec::new();
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'\\' {
            let octal = bytes.by_ref().take(3).collect::<Vec<_>>();
            if let Ok(text) = std::str::from_utf8(&octal) {
                if let Ok(value) = u8::from_str_radix(text, 8) {
                    result.push(value);
                    continue;
                }
            }
            return PathBuf::new();
        }
        result.push(byte);
    }
    std::ffi::OsString::from_vec(result).into()
}

/// # Errors
/// Refuses forged metadata, non-owned mounts, capacity expansion and broken FUSE.
#[allow(clippy::too_many_lines)] // The checks share one locked journal snapshot.
pub fn attest(root: &Path, workspace: &Path, ceiling: u64) -> Result<WorkspaceVolume> {
    ensure!(
        root.is_absolute() && ceiling >= 16 * 1024 * 1024,
        "invalid workspace authority selection"
    );
    private(root, true)?;
    private(&root.join("volumes.json"), false)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(root.join("volumes.lock"))?;
    private(&root.join("volumes.lock"), false)?;
    FileExt::try_lock_shared(&lock).context("workspace authority busy")?;
    ensure!(
        std::fs::metadata(root.join("volumes.json"))?.len() <= 1024 * 1024,
        "workspace journal too large"
    );
    let journal: Journal = serde_json::from_slice(&std::fs::read(root.join("volumes.json"))?)?;
    ensure!(
        journal.version == 1 && journal.volumes.len() <= 128,
        "unsupported workspace authority"
    );
    let mut roots = 0u64;
    for (id, volume) in &journal.volumes {
        ensure!(
            *id == volume.id
                && volume.image_bytes <= volume.subtree_bytes_max
                && volume.subtree_bytes_max <= journal.capacity_bytes
                && volume.image_bytes >= 16 * 1024 * 1024
                && volume.seed.as_ref().is_none_or(|value| value.len() <= 4096),
            "invalid workspace reservation"
        );
        let mut parent = volume.parent;
        let mut seen = std::collections::BTreeSet::from([*id]);
        while let Some(parent_id) = parent {
            ensure!(seen.insert(parent_id), "workspace ancestry cycle");
            parent = journal
                .volumes
                .get(&parent_id)
                .context("workspace ancestor missing")?
                .parent;
        }
        if volume.parent.is_none() {
            roots = roots
                .checked_add(volume.subtree_bytes_max)
                .context("workspace reservation overflow")?;
        }
        let descendants = journal
            .volumes
            .values()
            .filter(|child| child.parent == Some(*id))
            .try_fold(volume.image_bytes, |total, child| {
                total
                    .checked_add(child.subtree_bytes_max)
                    .context("workspace reservation overflow")
            })?;
        ensure!(
            descendants <= volume.subtree_bytes_max,
            "workspace parent capacity exceeded"
        );
    }
    ensure!(
        roots <= journal.capacity_bytes,
        "workspace owner capacity exceeded"
    );
    let root = root.canonicalize()?;
    let workspace = workspace.canonicalize()?;
    let volume = journal
        .volumes
        .values()
        .find(|volume| root.join(volume.id.to_string()).join("mount") == workspace)
        .context("workspace is not an owner-managed bounded volume")?;
    ensure!(
        volume.status == "ready" && volume.subtree_bytes_max <= ceiling,
        "workspace exceeds granted capacity or is not ready"
    );
    let directory = root.join(volume.id.to_string());
    private(&directory, true)?;
    let image = directory.join("workspace.ext4");
    private(&image, false)?;
    ensure!(
        std::fs::metadata(&image)?.len() == volume.image_bytes,
        "workspace backing size changed"
    );
    let mounts = std::fs::read_to_string("/proc/self/mountinfo")?;
    let found = mounts.lines().any(|line| {
        let Some((attributes, filesystem)) = line.split_once(" - ") else {
            return false;
        };
        let fields = attributes.split_whitespace().collect::<Vec<_>>();
        let filesystem = filesystem.split_whitespace().collect::<Vec<_>>();
        fields
            .get(4)
            .is_some_and(|path| unescape(path) == workspace)
            && filesystem.first() == Some(&"fuse.ext4")
            && filesystem
                .get(1)
                .is_some_and(|source| unescape(source) == image)
    });
    ensure!(found, "workspace mount identity is not verified by kernel");
    let stats = nix::sys::statvfs::statvfs(&workspace)?;
    let capacity = stats
        .blocks()
        .checked_mul(stats.fragment_size())
        .context("workspace capacity overflow")?;
    ensure!(
        capacity <= volume.image_bytes,
        "mounted workspace exceeds fixed backing capacity"
    );
    Ok(WorkspaceVolume {
        volume_id: volume.id,
        parent_volume_id: volume.parent,
        maximum_image_bytes: volume.image_bytes,
        subtree_maximum_bytes: volume.subtree_bytes_max,
        filesystem_capacity_bytes: capacity,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn mount_identity_preserves_unicode_and_escaped_whitespace() {
        assert_eq!(
            super::unescape("/tmp/कार्य\\040tree"),
            std::path::Path::new("/tmp/कार्य tree")
        );
        assert!(super::unescape("/tmp/invalid\\xyz").as_os_str().is_empty());
    }
}
