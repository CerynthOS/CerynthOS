//! Core library module for this `CerynthOS` component.
//!
//! Collects point-in-time system and scheduler telemetry from `/proc` and
//! `/sys` so higher layers (benchmarks, `cerynthd`, and eventually MIMIR)
//! can observe how the system behaves under different scheduling profiles.

use std::collections::HashMap;
use std::fs;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// CPU usage attributed to a single process over the last sampling interval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessCpuSample {
    pub pid: i32,
    pub command: String,
    pub cpu_percent: f64,
}

/// A single point-in-time snapshot of system and scheduler state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSnapshot {
    pub timestamp_ms: u128,
    pub cpu_usage_percent: f64,
    pub load_1m: f64,
    pub load_5m: f64,
    pub load_15m: f64,
    pub runnable_tasks: u64,
    pub total_tasks: u64,
    pub context_switches: u64,
    pub interrupts: u64,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
    pub top_processes: Vec<ProcessCpuSample>,
    pub scheduler_profile: Option<String>,
    pub sched_ext_state: String,
}

/// A source of [`SystemSnapshot`]s. `collect` is `&mut self` because
/// accurate CPU-usage percentages require differencing against the
/// previous sample.
pub trait TelemetrySource {
    fn collect(&mut self) -> Result<SystemSnapshot>;
}

#[derive(Debug, Clone, Copy, Default)]
struct CpuTimes {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
}

impl CpuTimes {
    fn from_fields(fields: &[u64]) -> Self {
        let f = |i: usize| fields.get(i).copied().unwrap_or(0);
        Self {
            user: f(0),
            nice: f(1),
            system: f(2),
            idle: f(3),
            iowait: f(4),
            irq: f(5),
            softirq: f(6),
            steal: f(7),
        }
    }

    fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    fn idle_total(&self) -> u64 {
        self.idle + self.iowait
    }

    fn usage_percent_since(&self, prev: &CpuTimes) -> f64 {
        let total_delta = self.total().saturating_sub(prev.total());
        let idle_delta = self.idle_total().saturating_sub(prev.idle_total());
        if total_delta == 0 {
            return 0.0;
        }
        let busy_delta = total_delta.saturating_sub(idle_delta);
        (busy_delta as f64 / total_delta as f64) * 100.0
    }
}

/// Reads live telemetry from `/proc` and `/sys` on a Linux host running
/// (or falling back from) a `sched_ext` scheduler.
pub struct ProcTelemetrySource {
    prev_cpu: Option<CpuTimes>,
    prev_proc_cpu: HashMap<i32, u64>,
    prev_sample_at: Option<Instant>,
    clk_tck: f64,
}

impl Default for ProcTelemetrySource {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcTelemetrySource {
    #[must_use]
    pub fn new() -> Self {
        // sysconf(_SC_CLK_TCK) is the only portable way to learn the
        // kernel's USER_HZ, which /proc/[pid]/stat and /proc/stat ticks
        // are expressed in; it cannot fail on Linux in practice.
        let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        Self {
            prev_cpu: None,
            prev_proc_cpu: HashMap::new(),
            prev_sample_at: None,
            clk_tck: if clk_tck > 0 { clk_tck as f64 } else { 100.0 },
        }
    }

    fn top_processes(&mut self, elapsed_secs: f64) -> Vec<ProcessCpuSample> {
        let mut samples = Vec::new();
        let mut seen = HashMap::new();

        let Ok(entries) = fs::read_dir("/proc") else {
            return samples;
        };

        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<i32>().ok())
            else {
                continue;
            };

            let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };

            let Some(open_paren) = stat.find('(') else {
                continue;
            };
            let Some(close_paren) = stat.rfind(')') else {
                continue;
            };
            let command = stat[open_paren + 1..close_paren].to_string();
            let rest: Vec<&str> = stat[close_paren + 1..].split_whitespace().collect();

            // Fields after "(comm)" are 1-indexed from state (field 3);
            // utime is field 14, stime is field 15.
            let utime: u64 = rest.get(11).and_then(|s| s.parse().ok()).unwrap_or(0);
            let stime: u64 = rest.get(12).and_then(|s| s.parse().ok()).unwrap_or(0);
            let ticks = utime + stime;

            seen.insert(pid, ticks);

            let prev_ticks = self.prev_proc_cpu.get(&pid).copied();
            if let Some(prev) = prev_ticks {
                if elapsed_secs > 0.0 {
                    let delta_ticks = ticks.saturating_sub(prev);
                    let cpu_percent = (delta_ticks as f64 / self.clk_tck) / elapsed_secs * 100.0;
                    if cpu_percent > 0.0 {
                        samples.push(ProcessCpuSample {
                            pid,
                            command,
                            cpu_percent,
                        });
                    }
                }
            }
        }

        self.prev_proc_cpu = seen;
        samples.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        samples.truncate(5);
        samples
    }
}

fn parse_meminfo(contents: &str) -> HashMap<String, u64> {
    contents
        .lines()
        .filter_map(|line| {
            let (key, rest) = line.split_once(':')?;
            let value_kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            Some((key.to_string(), value_kb))
        })
        .collect()
}

fn read_scheduler_profile() -> Option<String> {
    let contents = fs::read_to_string("/var/lib/cerynth/state.json").ok()?;
    let value: serde_json::Value = serde_json::from_str(&contents).ok()?;
    value
        .get("profile")
        .and_then(|p| p.as_str())
        .map(str::to_string)
}

impl TelemetrySource for ProcTelemetrySource {
    fn collect(&mut self) -> Result<SystemSnapshot> {
        let now = Instant::now();
        let elapsed_secs = self
            .prev_sample_at
            .map_or(0.0, |prev| now.duration_since(prev).as_secs_f64());

        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock before UNIX epoch")?
            .as_millis();

        let stat = fs::read_to_string("/proc/stat").context("reading /proc/stat")?;
        let mut context_switches = 0u64;
        let mut interrupts = 0u64;
        let mut runnable_tasks = 0u64;
        let mut cpu_cur = None;
        for line in stat.lines() {
            if let Some(rest) = line.strip_prefix("cpu ") {
                let fields: Vec<u64> = rest
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                cpu_cur = Some(CpuTimes::from_fields(&fields));
            } else if let Some(rest) = line.strip_prefix("ctxt ") {
                context_switches = rest.trim().parse().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("intr ") {
                interrupts = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("procs_running ") {
                runnable_tasks = rest.trim().parse().unwrap_or(0);
            }
        }

        let cpu_usage_percent = match (&self.prev_cpu, &cpu_cur) {
            (Some(prev), Some(cur)) => cur.usage_percent_since(prev),
            _ => 0.0,
        };
        self.prev_cpu = cpu_cur;

        let loadavg = fs::read_to_string("/proc/loadavg").context("reading /proc/loadavg")?;
        let load_parts: Vec<&str> = loadavg.split_whitespace().collect();
        let load_1m = load_parts
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let load_5m = load_parts
            .get(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let load_15m = load_parts
            .get(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let total_tasks = load_parts
            .get(3)
            .and_then(|s| s.split('/').nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let meminfo = fs::read_to_string("/proc/meminfo").context("reading /proc/meminfo")?;
        let mem = parse_meminfo(&meminfo);
        let kb = |key: &str| mem.get(key).copied().unwrap_or(0) * 1024;
        let memory_total_bytes = kb("MemTotal");
        let memory_used_bytes = memory_total_bytes.saturating_sub(kb("MemAvailable"));
        let swap_total_bytes = kb("SwapTotal");
        let swap_used_bytes = swap_total_bytes.saturating_sub(kb("SwapFree"));

        let sched_ext_state = fs::read_to_string("/sys/kernel/sched_ext/state")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unavailable".to_string());

        let top_processes = self.top_processes(elapsed_secs);
        self.prev_sample_at = Some(now);

        Ok(SystemSnapshot {
            timestamp_ms,
            cpu_usage_percent,
            load_1m,
            load_5m,
            load_15m,
            runnable_tasks,
            total_tasks,
            context_switches,
            interrupts,
            memory_used_bytes,
            memory_total_bytes,
            swap_used_bytes,
            swap_total_bytes,
            top_processes,
            scheduler_profile: read_scheduler_profile(),
            sched_ext_state,
        })
    }
}
