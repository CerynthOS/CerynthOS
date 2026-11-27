//! CPU topology discovery from `/sys/devices/system/cpu`.
//!
//! Kernel-maintained topology is the authority; no agent-reported values are
//! used. Missing topology files yield `None` rather than invented defaults.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A CPU topology snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuTopology {
    /// Logical CPU indices present on the machine.
    pub logical_cpus: Vec<u32>,
    /// Physical core IDs, one per logical CPU where available.
    pub core_ids: Vec<Option<u32>>,
    /// NUMA node IDs, one per logical CPU where available.
    pub node_ids: Vec<Option<u32>>,
    /// Sibling thread lists per logical CPU where available.
    pub thread_siblings: Vec<Vec<u32>>,
}

impl CpuTopology {
    /// Discover CPU topology from sysfs.
    ///
    /// # Errors
    /// Returns an error only if the sysfs root cannot be read at all.
    pub fn discover() -> Result<Self> {
        Self::discover_from(Path::new("/sys/devices/system/cpu"))
    }

    /// Discover topology from an explicit sysfs root (for tests/fixtures).
    pub fn discover_from(root: &Path) -> Result<Self> {
        let cpu_root = root.join("cpu");
        let mut logical_cpus = Vec::new();
        let mut core_ids = Vec::new();
        let mut node_ids = Vec::new();
        let mut thread_siblings = Vec::new();

        let entries =
            std::fs::read_dir(&cpu_root).with_context(|| format!("read {}", cpu_root.display()))?;
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(index) = name.strip_prefix("cpu").and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if !entry.path().join("topology").is_dir() {
                continue;
            }
            logical_cpus.push(index);
            core_ids.push(read_u32(&entry.path().join("topology/core_id")));
            node_ids.push(read_host_node(&entry.path().join("node")));
            thread_siblings.push(
                read_list(&entry.path().join("topology/thread_siblings_list")).unwrap_or_default(),
            );
        }
        logical_cpus.sort_unstable();

        Ok(Self {
            logical_cpus,
            core_ids,
            node_ids,
            thread_siblings,
        })
    }

    /// Number of logical CPUs observed.
    #[must_use]
    pub fn count(&self) -> usize {
        self.logical_cpus.len()
    }

    /// Whether any topology information was found.
    #[must_use]
    pub fn is_available(&self) -> bool {
        !self.logical_cpus.is_empty()
    }
}

fn read_u32(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_host_node(path: &Path) -> Option<u32> {
    if !path.is_dir() {
        return None;
    }
    let target = std::fs::read_link(path).ok()?;
    let name = target.file_name()?.to_str()?;
    name.strip_prefix("node")?.parse().ok()
}

fn read_list(path: &Path) -> Option<Vec<u32>> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut result = Vec::new();
    for part in text.trim().split(',') {
        if let Some((start, end)) = part.split_once('-') {
            let start: u32 = start.parse().ok()?;
            let end: u32 = end.parse().ok()?;
            result.extend(start..=end);
        } else {
            result.push(part.parse().ok()?);
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_from_fixture() {
        let root = std::env::temp_dir().join(format!("cpu-topo-{}", uuid::Uuid::new_v4()));
        let cpu0 = root.join("cpu/cpu0/topology");
        let cpu1 = root.join("cpu/cpu1/topology");
        std::fs::create_dir_all(&cpu0).unwrap();
        std::fs::create_dir_all(&cpu1).unwrap();
        std::fs::write(cpu0.join("core_id"), "0\n").unwrap();
        std::fs::write(cpu1.join("core_id"), "1\n").unwrap();
        std::fs::write(cpu0.join("thread_siblings_list"), "0\n").unwrap();
        std::fs::write(cpu1.join("thread_siblings_list"), "1\n").unwrap();

        let topo = CpuTopology::discover_from(&root).unwrap();
        assert_eq!(topo.logical_cpus, vec![0, 1]);
        assert_eq!(topo.core_ids, vec![Some(0), Some(1)]);
        assert_eq!(topo.thread_siblings, vec![vec![0], vec![1]]);
        assert_eq!(topo.count(), 2);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_root_yields_empty_topology() {
        let root = std::env::temp_dir().join(format!("cpu-empty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("cpu")).unwrap();
        let topo = CpuTopology::discover_from(&root).unwrap();
        assert!(!topo.is_available());
        let _ = std::fs::remove_dir_all(&root);
    }
}
