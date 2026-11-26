//! A single systemd writer for optional managed resource hierarchies.
use anyhow::{Context, Result, ensure};
use cerynth_agent_api::ResourceBudget;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Hierarchy {
    pub root: String,
    pub memory_max_bytes: u64,
    pub cpu_quota_micros_per_second: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Accounting {
    pub unit: String,
    pub control_group: String,
    pub memory_current_bytes: Option<u64>,
    pub memory_peak_bytes: Option<u64>,
    pub memory_max_bytes: Option<u64>,
    pub memory_events: std::collections::BTreeMap<String, u64>,
    pub cpu_stat: std::collections::BTreeMap<String, u64>,
    pub cpu_max: Option<String>,
    pub tasks_current: Option<u64>,
    pub memory_pressure: Option<String>,
    pub cpu_pressure: Option<String>,
}

fn hash(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))[..16].into()
}

impl Hierarchy {
    #[must_use]
    pub fn new(directory: &Path, memory_max_bytes: u64, cpu_quota_micros_per_second: u64) -> Self {
        Self {
            root: format!(
                "cerynth-r{}.slice",
                hash(directory.as_os_str().as_encoded_bytes())
            ),
            memory_max_bytes,
            cpu_quota_micros_per_second,
        }
    }

    #[must_use]
    pub fn project(&self, project: &str) -> String {
        format!(
            "{}-p{}.slice",
            self.root.trim_end_matches(".slice"),
            hash(project.as_bytes())
        )
    }

    /// # Errors
    /// Refuses unbounded hierarchies or an incompatible parent unit.
    pub fn session(&self, project: &str, parent: Option<&str>, id: Uuid) -> Result<String> {
        let project = self.project(project);
        let parent = parent.unwrap_or(&project);
        ensure!(
            (parent == project
                || parent.starts_with(&format!("{}-s", project.trim_end_matches(".slice"))))
                && valid_slice(parent),
            "parent resource slice is outside the project hierarchy"
        );
        let unit = format!(
            "{}-s{}.slice",
            parent.trim_end_matches(".slice"),
            id.simple()
        );
        ensure!(
            unit.len() <= 240,
            "kernel hierarchy depth/name limit exceeded"
        );
        Ok(unit)
    }

    /// Journalling must happen before this method: a timed-out systemd call may
    /// have created the unit even when acknowledgement was lost.
    /// # Errors
    /// Returns an error if systemd fails to create or validate exact limits.
    pub async fn ensure(
        &self,
        project: &str,
        session: &str,
        budget: &ResourceBudget,
    ) -> Result<()> {
        let global = ResourceBudget {
            memory_max_bytes: self.memory_max_bytes,
            memory_high_bytes: self.memory_max_bytes * 9 / 10,
            cpu_quota_micros_per_second: Some(self.cpu_quota_micros_per_second),
            tasks_max: 4096,
            ..ResourceBudget::default()
        };
        ensure_slice(&self.root, &global).await?;
        ensure_slice(&self.project(project), &global).await?;
        ensure_slice(session, budget).await?;
        Ok(())
    }
}

fn valid_slice(unit: &str) -> bool {
    unit.starts_with("cerynth-r")
        && unit.strip_suffix(".slice").is_some()
        && unit.len() <= 240
        && unit
            .trim_end_matches(".slice")
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

async fn command(program: &str, args: Vec<String>) -> Result<std::process::Output> {
    let uid = unsafe_free_uid()?;
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(program)
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path=/run/user/{uid}/bus"),
            )
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("systemd resource API deadline exceeded")??;
    ensure!(
        output.stdout.len() <= 64 * 1024 && output.stderr.len() <= 64 * 1024,
        "systemd response exceeds limit"
    );
    Ok(output)
}

// Read effective UID without adding a second platform-binding crate.
fn unsafe_free_uid() -> Result<u32> {
    std::fs::metadata("/proc/self")
        .map(|metadata| {
            use std::os::unix::fs::MetadataExt;
            metadata.uid()
        })
        .context("cannot read current process identity")
}

async fn property(unit: &str, key: &str) -> Result<String> {
    let output = command(
        "/usr/bin/systemctl",
        vec![
            "--user".into(),
            "show".into(),
            unit.into(),
            format!("--property={key}"),
            "--value".into(),
        ],
    )
    .await?;
    ensure!(
        output.status.success(),
        "systemd resource property unavailable"
    );
    Ok(String::from_utf8(output.stdout)?.trim().into())
}

async fn ensure_slice(unit: &str, budget: &ResourceBudget) -> Result<()> {
    ensure!(valid_slice(unit), "invalid managed resource unit");
    // systemd synthesizes any syntactically valid slice as "loaded" even
    // before it exists. Only Transient=yes identifies our configured unit.
    if property(unit, "Transient").await? != "yes" {
        ensure!(
            property(unit, "ActiveState").await? == "inactive"
                && property(unit, "FragmentPath").await?.is_empty(),
            "refusing to replace an externally owned slice"
        );
        let mut properties = vec![
            ("Description", "s", "Cerynth managed agent resources".into()),
            ("MemoryAccounting", "b", "true".into()),
            ("CPUAccounting", "b", "true".into()),
            ("TasksAccounting", "b", "true".into()),
            ("CPUWeight", "t", budget.cpu_weight.to_string()),
            ("MemoryHigh", "t", budget.memory_high_bytes.to_string()),
            ("MemoryMax", "t", budget.memory_max_bytes.to_string()),
            ("TasksMax", "t", budget.tasks_max.to_string()),
        ];
        if let Some(quota) = budget.cpu_quota_micros_per_second {
            properties.push(("CPUQuotaPerSecUSec", "t", quota.to_string()));
        }
        let mut args: Vec<String> = [
            "--user",
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "StartTransientUnit",
            "ssa(sv)a(sa(sv))",
            unit,
            "fail",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        args.push(properties.len().to_string());
        for (key, signature, value) in properties {
            args.extend([key.into(), signature.into(), value]);
        }
        args.push("0".into());
        let output = command("/usr/bin/busctl", args).await?;
        ensure!(
            output.status.success(),
            "systemd refused the managed resource slice"
        );
    }
    for (key, expected) in [
        ("MemoryMax", budget.memory_max_bytes),
        ("MemoryHigh", budget.memory_high_bytes),
        ("CPUWeight", u64::from(budget.cpu_weight)),
        ("TasksMax", u64::from(budget.tasks_max)),
    ] {
        let observed = property(unit, key).await?;
        ensure!(
            observed
                .parse::<u64>()
                .with_context(|| format!("invalid {key} response {observed:?}"))?
                == expected,
            "existing resource slice limits differ from owner budget"
        );
    }
    if let Some(expected) = budget.cpu_quota_micros_per_second {
        let path = cgroup_path(&property(unit, "ControlGroup").await?)?;
        let text = std::fs::read_to_string(path.join("cpu.max"))?;
        let values: Vec<_> = text.split_whitespace().collect();
        ensure!(
            values.len() == 2
                && values[0]
                    .parse::<u128>()
                    .with_context(|| format!("invalid kernel cpu.max {text:?}"))?
                    * 1_000_000
                    == u128::from(expected) * values[1].parse::<u128>()?,
            "kernel CPU quota differs from owner budget"
        );
    }
    Ok(())
}

/// # Errors
/// Only stops owned managed slice names and verifies the cgroup has no processes.
pub async fn stop(unit: &str) -> Result<()> {
    ensure!(valid_slice(unit), "invalid managed slice ownership");
    if property(unit, "LoadState").await? == "not-found" {
        return Ok(());
    }
    let output = command(
        "/usr/bin/systemctl",
        vec!["--user".into(), "stop".into(), unit.into()],
    )
    .await?;
    ensure!(
        output.status.success() || property(unit, "LoadState").await? == "not-found",
        "resource slice cleanup failed"
    );
    if property(unit, "LoadState").await? == "not-found" {
        return Ok(());
    }
    let group = property(unit, "ControlGroup").await?;
    if !group.is_empty() {
        let path = cgroup_path(&group)?;
        if path.exists() {
            let events = std::fs::read_to_string(path.join("cgroup.events"))?;
            ensure!(
                events.lines().any(|line| line == "populated 0"),
                "resource subtree still contains processes"
            );
        }
    }
    Ok(())
}

fn cgroup_path(group: &str) -> Result<PathBuf> {
    ensure!(
        group.starts_with('/') && !group.split('/').any(|s| s == ".." || s == "."),
        "invalid kernel cgroup path"
    );
    Ok(Path::new("/sys/fs/cgroup").join(group.trim_start_matches('/')))
}

/// Reads kernel aggregate counters, including all nested descendants.
/// # Errors
/// Rejects unavailable or untrusted unit/cgroup identities.
pub async fn accounting(unit: &str) -> Result<Accounting> {
    ensure!(valid_slice(unit), "invalid managed resource unit");
    let group = property(unit, "ControlGroup").await?;
    ensure!(!group.is_empty(), "resource cgroup is not active");
    let path = cgroup_path(&group)?;
    let number = |name| {
        std::fs::read_to_string(path.join(name))
            .ok()?
            .trim()
            .parse()
            .ok()
    };
    let text = |name| {
        std::fs::read_to_string(path.join(name))
            .ok()
            .map(|s| s.trim().to_owned())
    };
    let counters = |name| {
        text(name)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once(' ')?;
                Some((key.into(), value.parse().ok()?))
            })
            .collect()
    };
    Ok(Accounting {
        unit: unit.into(),
        control_group: group,
        memory_current_bytes: number("memory.current"),
        memory_peak_bytes: number("memory.peak"),
        memory_max_bytes: number("memory.max"),
        memory_events: counters("memory.events"),
        cpu_stat: counters("cpu.stat"),
        cpu_max: text("cpu.max"),
        tasks_current: number("pids.current"),
        memory_pressure: text("memory.pressure"),
        cpu_pressure: text("cpu.pressure"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slice_paths_bound_ancestry_and_do_not_accept_arbitrary_units() -> Result<()> {
        let hierarchy = Hierarchy::new(Path::new("/private/fixture"), 1_000_000, 1_000_000);
        let parent = hierarchy.session("project", None, Uuid::new_v4())?;
        let child = hierarchy.session("project", Some(&parent), Uuid::new_v4())?;
        assert!(child.starts_with(parent.trim_end_matches(".slice")));
        assert!(
            hierarchy
                .session("other", Some(&parent), Uuid::new_v4())
                .is_err()
        );
        assert!(!valid_slice("user.slice") && !valid_slice("cerynth-r../x.slice"));
        assert!(cgroup_path("/../root").is_err());
        Ok(())
    }
}
