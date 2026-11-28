//! Short-lived snapshot experiment; CPU quota/cgroup weight enforcement is not provided.
#[path = "../../cerynth-scx/src/policy.rs"]
mod baseline;
pub use baseline::{Load, order_tasks};
use crate::{QueuedTask, Profile};
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::{collections::BTreeMap, fs::OpenOptions, io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt}, sync::OnceLock};

static PREFERENCES: OnceLock<BTreeMap<i32, u32>> = OnceLock::new();

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot { version: u32, members: Vec<Member> }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    session_id: uuid::Uuid,
    pid: i32,
    start_time_ticks: u64,
    control_group: String,
    phase: cerynth_agent_api::WorkloadPhase,
}

pub fn initialize() -> Result<()> {
    let mut preferences = BTreeMap::new();
    if let Some(path) = std::env::var_os("CERYNTH_SCX_PHASE_SNAPSHOT") {
        let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
        let info = file.metadata()?;
        ensure!(info.is_file() && info.uid() == 0 && info.mode() & 0o777 == 0o600 && info.len() <= 131_072,
            "phase snapshot must be a private bounded root-owned regular file");
        let mut bytes = Vec::new(); file.take(131_073).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 131_072, "phase snapshot grew during read");
        let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
        ensure!(snapshot.version == 1 && snapshot.members.len() <= 128, "unsupported phase snapshot");
        for member in snapshot.members {
            ensure!(member.pid > 1 && !member.session_id.is_nil(), "invalid session process identity");
            let process = procfs::process::Process::new(member.pid)?;
            ensure!(process.stat()?.starttime == member.start_time_ticks, "phase process identity is stale");
            let cgroups = std::fs::read_to_string(format!("/proc/{}/cgroup", member.pid))?;
            ensure!(cgroups.lines().any(|line| line.strip_prefix("0::") == Some(member.control_group.as_str())), "phase cgroup witness differs");
            let weight = cerynth_resources::phase::preference(member.phase, 100, false);
            ensure!(preferences.insert(member.pid, weight).is_none(), "duplicate phase process");
        }
        eprintln!("phase snapshot: {} witnessed processes; static experiment, no CPU quota guarantee", preferences.len());
    }
    PREFERENCES.set(preferences).map_err(|_| anyhow::anyhow!("phase snapshot initialized twice"))?;
    Ok(())
}

pub fn slice_for(profile: Profile, task: &QueuedTask, load: Load) -> u64 {
    let base = baseline::slice_for(profile, task, load);
    PREFERENCES.get().and_then(|map| map.get(&task.pid))
        .map_or(base, |weight| adjusted_slice(base, *weight))
}

fn adjusted_slice(base: u64, weight: u32) -> u64 {
    (base.saturating_mul(u64::from(weight)) / 100).clamp(500_000, 20_000_000)
}

#[cfg(test)]
mod tests {
    #[test]
    fn phase_preference_has_a_progress_floor_and_bounded_slice() {
        assert_eq!(super::adjusted_slice(5_000_000, 100), 5_000_000);
        assert_eq!(super::adjusted_slice(5_000_000, 40), 2_000_000);
        assert_eq!(super::adjusted_slice(5_000_000, 0), 500_000);
        assert_eq!(super::adjusted_slice(u64::MAX, 100), 20_000_000);
    }
}
