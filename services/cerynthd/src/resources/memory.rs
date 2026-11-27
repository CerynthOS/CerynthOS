//! Machine memory information from `/proc/meminfo`.
//!
//! Values are read from the kernel; unavailable fields stay `None`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Machine-wide memory snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineMemory {
    /// Total physical memory in bytes.
    pub total_bytes: Option<u64>,
    /// Currently free memory in bytes.
    pub free_bytes: Option<u64>,
    /// Available memory (including reclaimable) in bytes.
    pub available_bytes: Option<u64>,
    /// Swap total in bytes.
    pub swap_total_bytes: Option<u64>,
    /// Swap free in bytes.
    pub swap_free_bytes: Option<u64>,
}

impl MachineMemory {
    /// Read memory information from `/proc/meminfo`.
    pub fn read() -> Result<Self> {
        Self::read_from(Path::new("/proc/meminfo"))
    }

    /// Read from an explicit meminfo path (for tests/fixtures).
    pub fn read_from(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let mut values = std::collections::BTreeMap::new();
        for line in text.lines() {
            let Some((key, rest)) = line.split_once(':') else {
                continue;
            };
            let Some(kib) = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok())
            else {
                continue;
            };
            values.insert(key.to_string(), kib.saturating_mul(1024));
        }
        Ok(Self {
            total_bytes: values.get("MemTotal").copied(),
            free_bytes: values.get("MemFree").copied(),
            available_bytes: values.get("MemAvailable").copied(),
            swap_total_bytes: values.get("SwapTotal").copied(),
            swap_free_bytes: values.get("SwapFree").copied(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meminfo_fixture() {
        let dir = std::env::temp_dir().join(format!("mem-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("meminfo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            "MemTotal:       16384000 kB\nMemFree:         8192000 kB\nMemAvailable:   12288000 kB\nSwapTotal:             0 kB\nSwapFree:              0 kB\n",
        )
        .unwrap();
        let mem = MachineMemory::read_from(&path).unwrap();
        assert_eq!(mem.total_bytes, Some(16384000 * 1024));
        assert_eq!(mem.free_bytes, Some(8192000 * 1024));
        assert_eq!(mem.available_bytes, Some(12288000 * 1024));
        assert_eq!(mem.swap_total_bytes, Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
