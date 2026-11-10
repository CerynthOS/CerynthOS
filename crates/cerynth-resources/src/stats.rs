//! Typed cgroup v2 usage and pressure statistics.
//!
//! Every parser here is a pure function so it can be tested against captured
//! kernel text without a writable cgroup.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current schema version of the resource usage snapshot.
pub const RESOURCE_USAGE_SCHEMA_VERSION: u32 = 1;

/// `cpu.stat` counters for one cgroup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuUsage {
    pub usage_usec: u64,
    pub user_usec: u64,
    pub system_usec: u64,
    pub nr_periods: u64,
    pub nr_throttled: u64,
    pub throttled_usec: u64,
}

/// `memory.events` counters for one cgroup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEvents {
    pub low: u64,
    pub high: u64,
    pub max: u64,
    pub oom: u64,
    pub oom_kill: u64,
    pub oom_group_kill: u64,
}

/// Memory usage and event counters for one cgroup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryUsage {
    pub current_bytes: u64,
    /// `memory.peak` is only present on newer kernels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_bytes: Option<u64>,
    /// The current `memory.high` value, if the file exists and is not `max`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high_bytes: Option<u64>,
    /// The current `memory.max` value, if the file exists and is not `max`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    pub events: MemoryEvents,
}

/// `pids.current` / `pids.max` for one cgroup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PidsUsage {
    pub current: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<u64>,
    /// Number of times the `pids.max` limit was hit (`pids.events: max`).
    pub max_events: u64,
}

/// One `io.stat` device line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IoDeviceUsage {
    pub device: String,
    pub rbytes: u64,
    pub wbytes: u64,
    pub rios: u64,
    pub wios: u64,
    pub dbytes: u64,
    pub dios: u64,
}

/// One pressure line's values (`avg10`, `avg60`, `avg300`, `total`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PressureValues {
    pub avg10: f64,
    pub avg60: f64,
    pub avg300: f64,
    pub total_us: u64,
}

/// The `some` and `full` lines from a `*.pressure` file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PressureLine {
    pub some: PressureValues,
    pub full: PressureValues,
}

/// CPU, memory and I/O pressure for one cgroup.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Pressure {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<PressureLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<PressureLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io: Option<PressureLine>,
}

/// A point-in-time snapshot of one session cgroup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceUsage {
    pub schema_version: u32,
    pub session_id: Uuid,
    pub cgroup_path: String,
    pub populated: bool,
    /// Every PID in the cgroup and its descendant cgroups.
    pub pids: Vec<i32>,
    pub cpu: CpuUsage,
    pub memory: MemoryUsage,
    pub pids_usage: PidsUsage,
    pub io: Vec<IoDeviceUsage>,
    pub pressure: Pressure,
}

/// Parses a file made of `key value` lines, such as `cpu.stat` or
/// `memory.events`.
#[must_use]
pub fn parse_keyed_u64(text: &str) -> BTreeMap<String, u64> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let key = parts.next()?;
            let value = parts.next()?.parse().ok()?;
            Some((key.to_string(), value))
        })
        .collect()
}

/// Parses `cpu.stat`.
#[must_use]
pub fn parse_cpu_stat(text: &str) -> CpuUsage {
    let values = parse_keyed_u64(text);
    CpuUsage {
        usage_usec: values.get("usage_usec").copied().unwrap_or(0),
        user_usec: values.get("user_usec").copied().unwrap_or(0),
        system_usec: values.get("system_usec").copied().unwrap_or(0),
        nr_periods: values.get("nr_periods").copied().unwrap_or(0),
        nr_throttled: values.get("nr_throttled").copied().unwrap_or(0),
        throttled_usec: values.get("throttled_usec").copied().unwrap_or(0),
    }
}

/// Parses `memory.events`.
#[must_use]
pub fn parse_memory_events(text: &str) -> MemoryEvents {
    let values = parse_keyed_u64(text);
    MemoryEvents {
        low: values.get("low").copied().unwrap_or(0),
        high: values.get("high").copied().unwrap_or(0),
        max: values.get("max").copied().unwrap_or(0),
        oom: values.get("oom").copied().unwrap_or(0),
        oom_kill: values.get("oom_kill").copied().unwrap_or(0),
        oom_group_kill: values.get("oom_group_kill").copied().unwrap_or(0),
    }
}

/// Parses `pids.events`.
#[must_use]
pub fn parse_pids_max_events(text: &str) -> u64 {
    parse_keyed_u64(text).get("max").copied().unwrap_or(0)
}

/// Parses a `*.pressure` file. Returns `None` when neither line is present.
#[must_use]
pub fn parse_pressure(text: &str) -> Option<PressureLine> {
    let mut line = PressureLine::default();
    let mut seen = false;
    for entry in text.lines() {
        if let Some(rest) = entry.strip_prefix("some ") {
            line.some = parse_pressure_values(rest);
            seen = true;
        } else if let Some(rest) = entry.strip_prefix("full ") {
            line.full = parse_pressure_values(rest);
            seen = true;
        }
    }
    seen.then_some(line)
}

fn parse_pressure_values(text: &str) -> PressureValues {
    let mut values = PressureValues::default();
    for token in text.split_whitespace() {
        let Some((key, raw)) = token.split_once('=') else {
            continue;
        };
        match key {
            "avg10" => values.avg10 = raw.parse().unwrap_or(0.0),
            "avg60" => values.avg60 = raw.parse().unwrap_or(0.0),
            "avg300" => values.avg300 = raw.parse().unwrap_or(0.0),
            "total" => values.total_us = raw.parse().unwrap_or(0),
            _ => {}
        }
    }
    values
}

/// Parses `io.stat`.
#[must_use]
pub fn parse_io_stat(text: &str) -> Vec<IoDeviceUsage> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let device = parts.next()?.to_string();
            let mut usage = IoDeviceUsage {
                device,
                rbytes: 0,
                wbytes: 0,
                rios: 0,
                wios: 0,
                dbytes: 0,
                dios: 0,
            };
            for token in parts {
                let Some((key, raw)) = token.split_once('=') else {
                    continue;
                };
                let Ok(value) = raw.parse::<u64>() else {
                    continue;
                };
                match key {
                    "rbytes" => usage.rbytes = value,
                    "wbytes" => usage.wbytes = value,
                    "rios" => usage.rios = value,
                    "wios" => usage.wios = value,
                    "dbytes" => usage.dbytes = value,
                    "dios" => usage.dios = value,
                    _ => {}
                }
            }
            Some(usage)
        })
        .collect()
}

/// Parses a `memory.high` / `memory.max` / `pids.max` value, where `max`
/// means unlimited.
#[must_use]
pub fn parse_limit(text: &str) -> Option<u64> {
    let text = text.trim();
    if text == "max" {
        None
    } else {
        text.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_stat() {
        let text = "\
usage_usec 123456
user_usec 90000
system_usec 33456
nr_periods 10
nr_throttled 2
throttled_usec 4000
";
        assert_eq!(
            parse_cpu_stat(text),
            CpuUsage {
                usage_usec: 123_456,
                user_usec: 90_000,
                system_usec: 33_456,
                nr_periods: 10,
                nr_throttled: 2,
                throttled_usec: 4_000,
            }
        );
    }

    #[test]
    fn parses_memory_events_and_tolerates_unknown_keys() {
        let text = "\
low 1
high 2
max 3
oom 4
oom_kill 5
oom_group_kill 6
future_counter 7
";
        assert_eq!(
            parse_memory_events(text),
            MemoryEvents {
                low: 1,
                high: 2,
                max: 3,
                oom: 4,
                oom_kill: 5,
                oom_group_kill: 6,
            }
        );
    }

    #[test]
    fn parses_pressure() {
        let text = "\
some avg10=1.50 avg60=0.25 avg300=0.01 total=1234
full avg10=0.75 avg60=0.10 avg300=0.00 total=567
";
        let pressure = parse_pressure(text).expect("pressure");
        assert!((pressure.some.avg10 - 1.5).abs() < f64::EPSILON);
        assert_eq!(pressure.some.total_us, 1234);
        assert!((pressure.full.avg60 - 0.1).abs() < f64::EPSILON);
        assert_eq!(pressure.full.total_us, 567);
    }

    #[test]
    fn missing_pressure_file_is_none() {
        assert_eq!(parse_pressure(""), None);
    }

    #[test]
    fn parses_io_stat() {
        let text = "8:0 rbytes=100 wbytes=200 rios=3 wios=4 dbytes=5 dios=6\n";
        assert_eq!(
            parse_io_stat(text),
            vec![IoDeviceUsage {
                device: "8:0".to_string(),
                rbytes: 100,
                wbytes: 200,
                rios: 3,
                wios: 4,
                dbytes: 5,
                dios: 6,
            }]
        );
    }

    #[test]
    fn parses_limits_and_max() {
        assert_eq!(parse_limit("1048576\n"), Some(1_048_576));
        assert_eq!(parse_limit("max\n"), None);
        assert_eq!(parse_limit("not-a-number"), None);
    }

    #[test]
    fn parses_pids_events() {
        assert_eq!(parse_pids_max_events("max 7\n"), 7);
        assert_eq!(parse_pids_max_events(""), 0);
    }
}
