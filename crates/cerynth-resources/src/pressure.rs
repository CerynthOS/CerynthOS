//! Versioned read-only kernel pressure input for resource and inference admission.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub version: u32,
    pub observed_at_ms: u64,
    pub available_memory_bytes: Option<u64>,
    pub memory_some_avg10: Option<f64>,
    pub memory_full_avg10: Option<f64>,
    pub cpu_some_avg10: Option<f64>,
    pub io_full_avg10: Option<f64>,
}

#[must_use]
pub fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

impl Sample {
    /// # Errors
    /// Rejects future/stale, nonfinite or out-of-range trusted input.
    pub fn validate(&self, now: u64) -> Result<()> {
        ensure!(
            self.version == 1
                && self.observed_at_ms <= now.saturating_add(1000)
                && now.saturating_sub(self.observed_at_ms) <= 5000,
            "pressure sample is stale or unsupported"
        );
        ensure!(
            [
                self.memory_some_avg10,
                self.memory_full_avg10,
                self.cpu_some_avg10,
                self.io_full_avg10
            ]
            .into_iter()
            .flatten()
            .all(|n| n.is_finite() && (0.0..=100.0).contains(&n)),
            "invalid PSI pressure value"
        );
        Ok(())
    }
    #[must_use]
    pub fn blocks_new_work(&self, now: u64) -> bool {
        self.validate(now).is_err()
            || self
                .available_memory_bytes
                .is_none_or(|n| n < 512 * 1024 * 1024)
            || self.memory_full_avg10.is_some_and(|n| n >= 5.0)
            || self.cpu_some_avg10.is_some_and(|n| n >= 95.0)
            || self.io_full_avg10.is_some_and(|n| n >= 10.0)
    }
}

fn psi(text: &str, kind: &str) -> Option<f64> {
    let line = text
        .lines()
        .find(|line| line.split_whitespace().next() == Some(kind))?;
    line.split_whitespace()
        .find_map(|field| field.strip_prefix("avg10=")?.parse().ok())
}

#[must_use]
pub fn sample() -> Sample {
    let memory = std::fs::read_to_string("/proc/pressure/memory").unwrap_or_default();
    let cpu = std::fs::read_to_string("/proc/pressure/cpu").unwrap_or_default();
    let io = std::fs::read_to_string("/proc/pressure/io").unwrap_or_default();
    let available = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("MemAvailable:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()?
                    .checked_mul(1024)
            })
        });
    Sample {
        version: 1,
        observed_at_ms: now_ms(),
        available_memory_bytes: available,
        memory_some_avg10: psi(&memory, "some"),
        memory_full_avg10: psi(&memory, "full"),
        cpu_some_avg10: psi(&cpu, "some"),
        io_full_avg10: psi(&io, "full"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_unknown_and_invalid_samples_do_not_admit_work() {
        assert_eq!(
            psi(
                "some avg10=12.34 avg60=4 total=5\nfull avg10=0.01 total=2",
                "some"
            ),
            Some(12.34)
        );
        let mut healthy = Sample {
            version: 1,
            observed_at_ms: 10_000,
            available_memory_bytes: Some(1024_u64.pow(3)),
            memory_some_avg10: Some(0.0),
            memory_full_avg10: Some(0.0),
            cpu_some_avg10: Some(0.0),
            io_full_avg10: None,
        };
        assert!(!healthy.blocks_new_work(10_000));
        assert!(healthy.blocks_new_work(15_001));
        healthy.memory_full_avg10 = Some(6.0);
        assert!(healthy.blocks_new_work(10_000));
        healthy.memory_full_avg10 = Some(f64::NAN);
        assert!(healthy.validate(10_000).is_err());
        healthy.memory_full_avg10 = None;
        healthy.available_memory_bytes = None;
        assert!(healthy.blocks_new_work(10_000));
    }
}
