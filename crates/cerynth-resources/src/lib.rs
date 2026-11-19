//! Accelerator monitoring and cooperative admission. No generic hard VRAM limits.
pub mod admission;
pub mod global;
pub mod platform;
pub mod pressure;
pub mod systemd;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementLevel {
    Monitoring,
    CooperativeReservation,
    EnforcedPartition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuDevice {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub driver: Option<String>,
    pub total_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    pub utilization_percent: Option<u32>,
    pub enforcement: EnforcementLevel,
    pub mig_status: String,
    pub mps_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuProcess {
    pub device_id: String,
    pub pid: u32,
    pub used_bytes: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HardwareInventory {
    pub logical_cpus: usize,
    pub cpu_model: Option<String>,
    pub memory_total_bytes: Option<u64>,
    pub kernel: String,
    pub gpus: Vec<GpuDevice>,
    pub gpu_processes: Vec<GpuProcess>,
    pub diagnostics: Vec<String>,
}

async fn command(program: &str, args: &[&str]) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(program)
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn mib(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok()?.checked_mul(1024 * 1024)
}

/// Discover available metrics. Missing metrics remain unavailable, never zero estimates.
///
/// # Errors
/// Returns an error only for invalid host inventory data.
pub async fn inventory() -> Result<HardwareInventory> {
    let mut gpus = Vec::new();
    let mut diagnostics = Vec::new();
    if let Some(csv) = command("nvidia-smi", &["--query-gpu=uuid,name,memory.total,memory.used,utilization.gpu,driver_version,mig.mode.current",
        "--format=csv,noheader,nounits"]).await {
        for line in csv.lines() {
            let f: Vec<_> = line.split(',').map(str::trim).collect();
            if f.len() != 7 { diagnostics.push("NVIDIA CSV row could not be parsed".into()); continue; }
            gpus.push(GpuDevice { id: f[0].into(), name: f[1].into(), vendor: "nvidia".into(),
                total_bytes: mib(f[2]), used_bytes: mib(f[3]), utilization_percent: f[4].parse().ok(),
                driver: Some(f[5].into()), enforcement: EnforcementLevel::Monitoring,
                mig_status: f[6].into(), mps_status: "not_configured_or_verified".into() });
        }
    } else { diagnostics.push("NVIDIA telemetry unavailable: missing tool, unsupported driver, or command timeout".into()); }
    gpus.extend(discover_drm(std::path::Path::new("/sys/class/drm")));
    let mut gpu_processes = Vec::new();
    if let Some(csv) = command(
        "nvidia-smi",
        &[
            "--query-compute-apps=gpu_uuid,pid,used_gpu_memory",
            "--format=csv,noheader,nounits",
        ],
    )
    .await
    {
        for line in csv.lines() {
            let fields: Vec<_> = line.split(',').map(str::trim).collect();
            if fields.len() == 3 {
                if let Ok(pid) = fields[1].parse() {
                    gpu_processes.push(GpuProcess {
                        device_id: fields[0].into(),
                        pid,
                        used_bytes: mib(fields[2]),
                    });
                }
            }
        }
    }
    let cpu_model = std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|s| {
        s.lines().find_map(|line| {
            line.strip_prefix("model name")
                .and_then(|line| line.split_once(':'))
                .map(|(_, v)| v.trim().into())
        })
    });
    let memory_total_bytes = total_memory();
    Ok(HardwareInventory {
        logical_cpus: std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
        cpu_model,
        memory_total_bytes,
        kernel: std::fs::read_to_string("/proc/sys/kernel/osrelease")?
            .trim()
            .into(),
        gpus,
        gpu_processes,
        diagnostics,
    })
}

/// AMD/Intel DRM fields are read as bytes/percent; absent metrics remain unknown.
#[must_use]
pub fn discover_drm(root: &std::path::Path) -> Vec<GpuDevice> {
    let mut gpus = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let path = entry.path().join("device");
            let vendor = std::fs::read_to_string(path.join("vendor")).unwrap_or_default();
            if vendor.trim() != "0x1002" && vendor.trim() != "0x8086" {
                continue;
            }
            let read = |file: &str| {
                std::fs::read_to_string(path.join(file))
                    .ok()?
                    .trim()
                    .parse::<u64>()
                    .ok()
            };
            gpus.push(GpuDevice {
                id: name.clone(),
                name,
                vendor: if vendor.trim() == "0x1002" {
                    "amd"
                } else {
                    "intel"
                }
                .into(),
                driver: std::fs::read_link(path.join("driver"))
                    .ok()
                    .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned())),
                total_bytes: read("mem_info_vram_total"),
                used_bytes: read("mem_info_vram_used"),
                utilization_percent: read("gpu_busy_percent")
                    .filter(|n| *n <= 100)
                    .and_then(|n| u32::try_from(n).ok()),
                enforcement: EnforcementLevel::Monitoring,
                mig_status: "not_applicable".into(),
                mps_status: "not_applicable".into(),
            });
        }
    }
    gpus
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Admitted,
    Completed,
    Cancelled,
    Failed,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuRequest {
    pub session_id: Uuid,
    pub device_id: String,
    pub memory_bytes: u64,
    pub priority: i32,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuJob {
    pub id: Uuid,
    pub request: GpuRequest,
    pub state: JobState,
    pub sequence: u64,
    pub admitted_at_ms: Option<u64>,
    pub enforcement: EnforcementLevel,
    #[serde(default)]
    pub supervised: bool,
    #[serde(default)]
    pub cleanup_pending: bool,
}

#[derive(Default)]
pub struct GpuBroker {
    pressure_blocked: bool,
    pub devices: BTreeMap<String, GpuDevice>,
    pub jobs: BTreeMap<Uuid, GpuJob>,
    pub desktop_reserve_bytes: u64,
    sequence: u64,
}

impl GpuBroker {
    pub fn set_pressure(&mut self, blocked: bool) {
        self.pressure_blocked = blocked;
    }
    #[must_use]
    pub fn new(desktop_reserve_bytes: u64) -> Self {
        Self {
            desktop_reserve_bytes,
            ..Self::default()
        }
    }
    /// Refresh telemetry without discarding reservations.
    pub fn update_devices(&mut self, devices: Vec<GpuDevice>) {
        self.devices = devices
            .into_iter()
            .map(|device| (device.id.clone(), device))
            .collect();
    }

    /// Submit an already authorized request. The caller must check session capability/budget.
    ///
    /// # Errors
    /// Returns an error for unsupported devices, unknown capacity, or impossible budgets.
    pub fn submit(&mut self, request: GpuRequest, now_ms: u64) -> Result<GpuJob> {
        ensure!(self.jobs.len() < 1024, "GPU job registry full");
        let device = self
            .devices
            .get(&request.device_id)
            .ok_or_else(|| anyhow::anyhow!("unknown GPU"))?;
        let total = device
            .total_bytes
            .ok_or_else(|| anyhow::anyhow!("GPU memory capacity unavailable"))?;
        ensure!(
            device.used_bytes.is_some(),
            "GPU usage unavailable; cooperative admission disabled"
        );
        ensure!(
            request.memory_bytes > 0
                && request.memory_bytes <= total.saturating_sub(self.desktop_reserve_bytes)
                && request.timeout_seconds > 0
                && request.timeout_seconds <= 86400
                && (-100..=100).contains(&request.priority),
            "invalid GPU job budget"
        );
        self.sequence += 1;
        let id = Uuid::new_v4();
        self.jobs.insert(
            id,
            GpuJob {
                id,
                request,
                state: JobState::Queued,
                sequence: self.sequence,
                admitted_at_ms: None,
                enforcement: EnforcementLevel::CooperativeReservation,
                supervised: false,
                cleanup_pending: false,
            },
        );
        self.schedule(now_ms);
        Ok(self.jobs[&id].clone())
    }

    pub fn schedule(&mut self, now_ms: u64) {
        for job in self.jobs.values_mut() {
            if job.state == JobState::Admitted
                && !job.cleanup_pending
                && job.admitted_at_ms.is_some_and(|start| {
                    now_ms.saturating_sub(start) >= job.request.timeout_seconds.saturating_mul(1000)
                })
            {
                job.state = JobState::Expired;
            }
        }
        if self.pressure_blocked {
            return;
        }
        let mut queued: Vec<_> = self
            .jobs
            .values()
            .filter(|j| j.state == JobState::Queued)
            .map(|j| (j.request.priority, j.sequence, j.id))
            .collect();
        queued.sort_by_key(|(priority, sequence, _)| (std::cmp::Reverse(*priority), *sequence));
        for (_, _, id) in queued {
            let job = &self.jobs[&id];
            let Some(device) = self.devices.get(&job.request.device_id) else {
                continue;
            };
            let (Some(total), Some(used)) = (device.total_bytes, device.used_bytes) else {
                continue;
            };
            let reserved = self
                .jobs
                .values()
                .filter(|j| j.state == JobState::Admitted && j.request.device_id == device.id)
                .fold(0_u64, |sum, job| {
                    sum.saturating_add(job.request.memory_bytes)
                });
            // Conservative: measured usage and reservations may overlap. Never subtract
            // unknown usage as if it were ours, or claim a driver-enforced partition.
            let available = total
                .saturating_sub(used)
                .saturating_sub(self.desktop_reserve_bytes)
                .saturating_sub(reserved);
            if job.request.memory_bytes <= available {
                if let Some(job) = self.jobs.get_mut(&id) {
                    job.state = JobState::Admitted;
                    job.admitted_at_ms = Some(now_ms);
                }
            }
        }
    }

    /// Release cooperative accounting. Does not terminate an arbitrary CUDA process.
    ///
    /// # Errors
    /// Returns an error for unknown jobs or invalid state changes.
    pub fn finish(&mut self, id: Uuid, state: JobState, now_ms: u64) -> Result<GpuJob> {
        ensure!(
            self.jobs.get(&id).is_some_and(|job| !job.supervised),
            "supervised GPU jobs require verified process cleanup"
        );
        self.finalize(id, state, now_ms)
    }

    /// Bind an admitted lease to supervisor-owned execution. A live or unknown
    /// process keeps its reservation even after the nominal lease deadline.
    /// # Errors
    /// Rejects expired, queued, duplicate or unknown leases.
    pub fn bind_process(&mut self, id: Uuid, now_ms: u64) -> Result<GpuJob> {
        ensure!(
            !self.pressure_blocked,
            "new GPU execution is blocked by resource pressure"
        );
        self.schedule(now_ms);
        let job = self
            .jobs
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("unknown GPU job"))?;
        ensure!(
            job.state == JobState::Admitted && !job.supervised,
            "GPU lease is not available for execution"
        );
        job.supervised = true;
        job.cleanup_pending = true;
        job.admitted_at_ms = Some(now_ms);
        Ok(job.clone())
    }

    /// Called only after the supervisor verifies its owned execution boundary is empty.
    /// # Errors
    /// Rejects ordinary reservations and already completed jobs.
    pub fn finish_supervised(&mut self, id: Uuid, state: JobState, now_ms: u64) -> Result<GpuJob> {
        ensure!(
            self.jobs.get(&id).is_some_and(|job| job.supervised),
            "GPU job is not supervised"
        );
        self.finalize(id, state, now_ms)
    }

    fn finalize(&mut self, id: Uuid, state: JobState, now_ms: u64) -> Result<GpuJob> {
        ensure!(
            matches!(
                state,
                JobState::Completed | JobState::Cancelled | JobState::Failed
            ),
            "invalid final GPU state"
        );
        let job = self
            .jobs
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("unknown GPU job"))?;
        ensure!(
            matches!(job.state, JobState::Queued | JobState::Admitted),
            "GPU job already terminal"
        );
        job.state = state;
        job.cleanup_pending = false;
        let result = job.clone();
        self.schedule(now_ms);
        Ok(result)
    }
}

#[must_use]
pub fn total_memory() -> Option<u64> {
    std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
        s.lines()
            .find_map(|line| line.strip_prefix("MemTotal:"))
            .and_then(|n| n.split_whitespace().next())
            .and_then(|n| n.parse::<u64>().ok())
            .and_then(|kb| kb.checked_mul(1024))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amd_and_intel_drm_adapters_keep_missing_and_malformed_metrics_unknown() -> Result<()> {
        let root = tempfile::tempdir()?;
        for (card, vendor, driver) in [
            ("card0", "0x1002", "amdgpu"),
            ("card1", "0x8086", "i915"),
            ("card0-HDMI-A-1", "0x1002", "amdgpu"),
        ] {
            let device = root.path().join(card).join("device");
            std::fs::create_dir_all(&device)?;
            std::fs::write(device.join("vendor"), vendor)?;
            std::os::unix::fs::symlink(
                format!("/sys/bus/pci/drivers/{driver}"),
                device.join("driver"),
            )?;
        }
        let amd = root.path().join("card0/device");
        std::fs::write(
            amd.join("mem_info_vram_total"),
            (16_u64 * 1024_u64.pow(3)).to_string(),
        )?;
        std::fs::write(
            amd.join("mem_info_vram_used"),
            (4_u64 * 1024_u64.pow(3)).to_string(),
        )?;
        std::fs::write(amd.join("gpu_busy_percent"), "42")?;
        let devices = discover_drm(root.path());
        assert_eq!(devices.len(), 2);
        let amd_device = devices.iter().find(|d| d.vendor == "amd").unwrap();
        assert_eq!(amd_device.total_bytes, Some(16 * 1024_u64.pow(3)));
        assert_eq!(amd_device.utilization_percent, Some(42));
        assert_eq!(amd_device.driver.as_deref(), Some("amdgpu"));
        let intel = devices.iter().find(|d| d.vendor == "intel").unwrap();
        assert!(
            intel.total_bytes.is_none()
                && intel.used_bytes.is_none()
                && intel.utilization_percent.is_none()
        );
        std::fs::write(amd.join("mem_info_vram_used"), "N/A")?;
        std::fs::write(amd.join("gpu_busy_percent"), "150")?;
        let devices = discover_drm(root.path());
        let amd = devices.iter().find(|d| d.vendor == "amd").unwrap();
        assert!(amd.used_bytes.is_none() && amd.utilization_percent.is_none());
        Ok(())
    }

    #[test]
    fn admission_respects_desktop_reserve_usage_priority_and_expiration() -> Result<()> {
        let device = GpuDevice {
            id: "gpu".into(),
            name: "fixture".into(),
            vendor: "fixture".into(),
            driver: None,
            total_bytes: Some(100),
            used_bytes: Some(10),
            utilization_percent: None,
            enforcement: EnforcementLevel::Monitoring,
            mig_status: "unsupported".into(),
            mps_status: "unsupported".into(),
        };
        let mut broker = GpuBroker {
            desktop_reserve_bytes: 20,
            ..GpuBroker::default()
        };
        broker.update_devices(vec![device]);
        let request = |memory, priority| GpuRequest {
            session_id: Uuid::new_v4(),
            device_id: "gpu".into(),
            memory_bytes: memory,
            priority,
            timeout_seconds: 1,
        };
        let first = broker.submit(request(50, 0), 0)?;
        assert_eq!(first.state, JobState::Admitted);
        let low = broker.submit(request(40, 1), 0)?;
        let high = broker.submit(request(60, 10), 0)?;
        assert_eq!(low.state, JobState::Queued);
        assert!(broker.submit(request(81, 0), 0).is_err());
        broker.finish(first.id, JobState::Completed, 1)?;
        assert_eq!(broker.jobs[&high.id].state, JobState::Admitted);
        assert_eq!(broker.jobs[&low.id].state, JobState::Queued);
        broker.schedule(1002);
        assert_eq!(broker.jobs[&high.id].state, JobState::Expired);
        assert_eq!(broker.jobs[&low.id].state, JobState::Admitted);
        Ok(())
    }

    #[test]
    fn owned_execution_cannot_expire_or_be_manually_released_before_cleanup() -> Result<()> {
        let mut broker = GpuBroker::new(20);
        broker.update_devices(vec![GpuDevice {
            id: "gpu".into(),
            name: "fixture".into(),
            vendor: "fixture".into(),
            driver: None,
            total_bytes: Some(100),
            used_bytes: Some(10),
            utilization_percent: None,
            enforcement: EnforcementLevel::Monitoring,
            mig_status: "unsupported".into(),
            mps_status: "unsupported".into(),
        }]);
        let request = GpuRequest {
            session_id: Uuid::new_v4(),
            device_id: "gpu".into(),
            memory_bytes: 50,
            priority: 0,
            timeout_seconds: 1,
        };
        let owned = broker.submit(request.clone(), 0)?;
        broker.bind_process(owned.id, 0)?;
        assert!(broker.bind_process(owned.id, 0).is_err());
        let next = broker.submit(request, 1)?;
        broker.schedule(5000);
        assert_eq!(broker.jobs[&owned.id].state, JobState::Admitted);
        assert_eq!(broker.jobs[&next.id].state, JobState::Queued);
        assert!(broker.finish(owned.id, JobState::Completed, 5001).is_err());
        broker.finish_supervised(owned.id, JobState::Cancelled, 5002)?;
        assert!(!broker.jobs[&owned.id].cleanup_pending);
        assert_eq!(broker.jobs[&next.id].state, JobState::Admitted);
        Ok(())
    }
}
