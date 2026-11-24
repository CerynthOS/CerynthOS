//! Advanced controls applied through the existing systemd owner, never raw writes.
use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{DeviceIoLimit, PlatformLimits};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
};

pub(crate) type Property = (String, String, Vec<String>);

/// # Errors
/// Rejects malformed kernel CPU lists.
pub fn cpu_list(text: &str) -> Result<BTreeSet<u32>> {
    let mut result = BTreeSet::new();
    for range in text.trim().split(',').filter(|s| !s.is_empty()) {
        let (first, last) = range.split_once('-').unwrap_or((range, range));
        let first = first.parse::<u32>()?;
        let last = last.parse::<u32>()?;
        ensure!(
            first <= last && last <= 4095,
            "CPU list exceeds supported range"
        );
        result.extend(first..=last);
    }
    Ok(result)
}

fn device_path(device: &DeviceIoLimit) -> Result<PathBuf> {
    let name = format!("{}:{}", device.major, device.minor);
    ensure!(
        !Path::new(&format!("/sys/dev/block/{name}/partition")).exists(),
        "I/O limits must name the whole backing block device, not a partition"
    );
    let direct = PathBuf::from(format!("/dev/block/{name}"));
    let path = if direct.exists() {
        direct
    } else {
        let text = std::fs::read_to_string(format!("/sys/dev/block/{name}/uevent"))?;
        let name = text
            .lines()
            .find_map(|line| line.strip_prefix("DEVNAME="))
            .context("block device has no kernel name")?;
        ensure!(
            !name.starts_with('/')
                && !name.split('/').any(|part| part == ".." || part == ".")
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"/_-".contains(&c)),
            "invalid block device name"
        );
        Path::new("/dev").join(name)
    };
    let metadata = std::fs::metadata(&path)?;
    ensure!(
        metadata.file_type().is_block_device()
            && nix::libc::major(metadata.rdev()) == device.major
            && nix::libc::minor(metadata.rdev()) == device.minor,
        "device identity mismatch"
    );
    Ok(path)
}

pub(crate) fn properties(limits: &PlatformLimits) -> Result<Vec<Property>> {
    limits.validate().map_err(anyhow::Error::msg)?;
    let mut result = Vec::new();
    if let Some(value) = limits.memory_swap_max_bytes {
        result.push(("MemorySwapMax".into(), "t".into(), vec![value.to_string()]));
    }
    if let Some(value) = limits.io_weight {
        result.push(("IOWeight".into(), "t".into(), vec![value.to_string()]));
    }
    if limits.io_weight.is_some() || !limits.io_devices.is_empty() {
        result.push(("IOAccounting".into(), "b".into(), vec!["true".into()]));
    }
    if let Some(cpus) = &limits.allowed_cpus {
        let online = cpu_list(&std::fs::read_to_string("/sys/devices/system/cpu/online")?)?;
        ensure!(
            cpus.is_subset(&online),
            "requested CPU is offline or unavailable"
        );
        let mut mask = vec![0u8; usize::try_from(*cpus.last().context("empty cpuset")?)? / 8 + 1];
        for cpu in cpus {
            mask[usize::try_from(*cpu)? / 8] |= 1 << (cpu % 8);
        }
        let mut values = vec![mask.len().to_string()];
        values.extend(mask.iter().map(u8::to_string));
        result.push(("AllowedCPUs".into(), "ay".into(), values));
    }
    for (name, rate) in [
        ("IOReadBandwidthMax", 0),
        ("IOWriteBandwidthMax", 1),
        ("IOReadIOPSMax", 2),
        ("IOWriteIOPSMax", 3),
    ] {
        let mut entries = Vec::new();
        for device in &limits.io_devices {
            if let Some(value) = [
                device.read_bytes_per_second,
                device.write_bytes_per_second,
                device.read_iops,
                device.write_iops,
            ][rate]
            {
                entries.extend([
                    device_path(device)?.to_string_lossy().into_owned(),
                    value.to_string(),
                ]);
            }
        }
        if !entries.is_empty() {
            let mut values = vec![(entries.len() / 2).to_string()];
            values.extend(entries);
            result.push((name.into(), "a(st)".into(), values));
        }
    }
    Ok(result)
}

/// # Errors
/// Rejects unknown devices and invalid constraints before process creation.
pub fn scope_properties(limits: &PlatformLimits) -> Result<Vec<String>> {
    let mut result = Vec::new();
    // Resolve devices and offline CPU requests before giving systemd any work.
    properties(limits)?;
    if let Some(value) = limits.memory_swap_max_bytes {
        result.push(format!("--property=MemorySwapMax={value}"));
    }
    if let Some(value) = limits.io_weight {
        result.push(format!("--property=IOWeight={value}"));
    }
    if let Some(cpus) = &limits.allowed_cpus {
        result.push(format!(
            "--property=AllowedCPUs={}",
            cpus.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    for device in &limits.io_devices {
        for (key, value) in [
            ("IOReadBandwidthMax", device.read_bytes_per_second),
            ("IOWriteBandwidthMax", device.write_bytes_per_second),
            ("IOReadIOPSMax", device.read_iops),
            ("IOWriteIOPSMax", device.write_iops),
        ] {
            if let Some(value) = value {
                result.push(format!(
                    "--property={key}={} {value}",
                    device_path(device)?.display()
                ));
            }
        }
    }
    Ok(result)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Observed {
    pub memory_swap_max_bytes: Option<u64>,
    pub memory_swap_current_bytes: Option<u64>,
    pub allowed_cpus: Option<BTreeSet<u32>>,
    pub effective_cpus: Option<BTreeSet<u32>>,
    pub io_weight: Option<String>,
    pub io_max: Option<String>,
}

#[must_use]
pub fn observe(path: &Path) -> Observed {
    let read = |name| {
        std::fs::read_to_string(path.join(name))
            .ok()
            .map(|s| s.trim().to_owned())
    };
    Observed {
        memory_swap_max_bytes: read("memory.swap.max").and_then(|s| s.parse().ok()),
        memory_swap_current_bytes: read("memory.swap.current").and_then(|s| s.parse().ok()),
        allowed_cpus: read("cpuset.cpus").and_then(|s| cpu_list(&s).ok()),
        effective_cpus: read("cpuset.cpus.effective").and_then(|s| cpu_list(&s).ok()),
        io_weight: read("io.weight"),
        io_max: read("io.max"),
    }
}

pub(crate) fn verify(path: &Path, limits: &PlatformLimits) -> Result<()> {
    let observed = observe(path);
    if let Some(expected) = limits.memory_swap_max_bytes {
        ensure!(
            observed.memory_swap_max_bytes == Some(expected),
            "swap limit not enforced by kernel"
        );
    }
    if let Some(expected) = &limits.allowed_cpus {
        ensure!(
            observed.allowed_cpus.as_ref() == Some(expected)
                && observed.effective_cpus.as_ref() == Some(expected),
            "CPU affinity is unavailable or narrowed by an ancestor"
        );
    }
    if let Some(expected) = limits.io_weight {
        ensure!(
            observed.io_weight.as_ref().is_some_and(|text| text
                .lines()
                .any(|line| line == format!("default {expected}"))),
            "I/O weight not applied; device fairness still depends on hardware scheduler"
        );
    }
    for device in &limits.io_devices {
        let prefix = format!("{}:{} ", device.major, device.minor);
        let line = observed
            .io_max
            .as_deref()
            .and_then(|text| text.lines().find_map(|line| line.strip_prefix(&prefix)))
            .context("device throughput controller unavailable")?;
        for (key, expected) in [
            ("rbps", device.read_bytes_per_second),
            ("wbps", device.write_bytes_per_second),
            ("riops", device.read_iops),
            ("wiops", device.write_iops),
        ] {
            if let Some(expected) = expected {
                ensure!(
                    line.split_whitespace()
                        .any(|token| token == format!("{key}={expected}")),
                    "device throughput limit not applied"
                );
            }
        }
    }
    Ok(())
}
