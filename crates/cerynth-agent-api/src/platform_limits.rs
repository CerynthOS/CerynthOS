//! Optional platform constraints. Missing fields never establish enforcement.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_swap_max_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_cpus: Option<BTreeSet<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io_weight: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub io_devices: Vec<DeviceIoLimit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceIoLimit {
    pub major: u32,
    pub minor: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_bytes_per_second: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_bytes_per_second: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_iops: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_iops: Option<u64>,
}

fn bounded<T: PartialOrd>(child: Option<T>, parent: Option<T>) -> bool {
    match (child, parent) {
        (_, None) => true,
        (Some(child), Some(parent)) => child <= parent,
        (None, Some(_)) => false,
    }
}

impl PlatformLimits {
    /// Materialize inherited ceilings before validating a delegated request.
    pub fn inherit_from(&mut self, parent: &Self) {
        self.memory_swap_max_bytes = self.memory_swap_max_bytes.or(parent.memory_swap_max_bytes);
        self.allowed_cpus = self
            .allowed_cpus
            .take()
            .or_else(|| parent.allowed_cpus.clone());
        self.io_weight = self.io_weight.or(parent.io_weight);
        for limit in &parent.io_devices {
            if let Some(child) = self
                .io_devices
                .iter_mut()
                .find(|child| child.major == limit.major && child.minor == limit.minor)
            {
                child.read_bytes_per_second =
                    child.read_bytes_per_second.or(limit.read_bytes_per_second);
                child.write_bytes_per_second = child
                    .write_bytes_per_second
                    .or(limit.write_bytes_per_second);
                child.read_iops = child.read_iops.or(limit.read_iops);
                child.write_iops = child.write_iops.or(limit.write_iops);
            } else {
                self.io_devices.push(limit.clone());
            }
        }
    }
    /// # Errors
    /// Rejects invalid CPU sets, weights, zero rates and duplicate devices.
    pub fn validate(&self) -> Result<(), String> {
        if self.allowed_cpus.as_ref().is_some_and(|cpus| {
            cpus.is_empty() || cpus.len() > 4096 || cpus.last().is_some_and(|cpu| *cpu > 4095)
        }) || self
            .io_weight
            .is_some_and(|weight| !(1..=10_000).contains(&weight))
            || self.io_devices.len() > 16
        {
            return Err("invalid advanced platform limits".into());
        }
        let mut devices = BTreeSet::new();
        for device in &self.io_devices {
            let rates = [
                device.read_bytes_per_second,
                device.write_bytes_per_second,
                device.read_iops,
                device.write_iops,
            ];
            if !devices.insert((device.major, device.minor))
                || rates.iter().all(Option::is_none)
                || rates.contains(&Some(0))
            {
                return Err("invalid or duplicate I/O device constraint".into());
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn fits_within(&self, parent: &Self) -> bool {
        bounded(self.memory_swap_max_bytes, parent.memory_swap_max_bytes)
            && bounded(self.io_weight, parent.io_weight)
            && match (&self.allowed_cpus, &parent.allowed_cpus) {
                (_, None) => true,
                (Some(child), Some(parent)) => child.is_subset(parent),
                (None, Some(_)) => false,
            }
            && parent.io_devices.iter().all(|parent| {
                self.io_devices.iter().any(|child| {
                    child.major == parent.major
                        && child.minor == parent.minor
                        && bounded(child.read_bytes_per_second, parent.read_bytes_per_second)
                        && bounded(child.write_bytes_per_second, parent.write_bytes_per_second)
                        && bounded(child.read_iops, parent.read_iops)
                        && bounded(child.write_iops, parent.write_iops)
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delegated_controls_inherit_and_cannot_expand_authority() {
        let parent = PlatformLimits {
            memory_swap_max_bytes: Some(0),
            allowed_cpus: Some([0, 1].into()),
            io_weight: Some(100),
            io_devices: vec![DeviceIoLimit {
                major: 8,
                minor: 0,
                read_bytes_per_second: Some(1024),
                write_bytes_per_second: None,
                read_iops: None,
                write_iops: None,
            }],
        };
        let mut child = PlatformLimits::default();
        child.inherit_from(&parent);
        assert!(child.fits_within(&parent));
        child.allowed_cpus.as_mut().unwrap().insert(2);
        assert!(!child.fits_within(&parent));
        child = parent.clone();
        child.memory_swap_max_bytes = Some(1);
        assert!(!child.fits_within(&parent));
        child = parent.clone();
        child.io_devices[0].read_bytes_per_second = Some(2048);
        assert!(!child.fits_within(&parent));
        child = parent.clone();
        child.io_devices[0].read_bytes_per_second = Some(512);
        assert!(child.fits_within(&parent));
    }
    #[test]
    fn legacy_budget_serialization_omits_advanced_constraints() {
        let budget = crate::ResourceBudget::default();
        let value = serde_json::to_value(&budget).unwrap();
        assert!(value.get("platform").is_none());
        assert_eq!(
            serde_json::from_value::<crate::ResourceBudget>(value).unwrap(),
            budget
        );
        assert!(
            PlatformLimits {
                allowed_cpus: Some(BTreeSet::new()),
                ..PlatformLimits::default()
            }
            .validate()
            .is_err()
        );
    }
}
