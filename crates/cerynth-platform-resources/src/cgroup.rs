//! cgroup v2 discovery and per-session subtree management.
//!
//! The production daemon runs as root and creates session cgroups directly.
//! Tests and unprivileged callers can instead [`SessionCgroup::open`] an
//! existing cgroup, such as a systemd transient scope.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use uuid::Uuid;

use crate::budget::{AppliedLimit, AppliedLimits, Enforcement, ResourceLimits};
use crate::stats::{
    RESOURCE_USAGE_SCHEMA_VERSION, ResourceUsage, parse_cpu_stat, parse_io_stat, parse_limit,
    parse_memory_events, parse_pids_max_events, parse_pressure,
};

/// Conventional cgroup v2 mount point.
pub const CGROUP2_MOUNT: &str = "/sys/fs/cgroup";

/// Prefix used for per-session cgroup directories.
pub const SESSION_PREFIX: &str = "cerynth-session-";

/// Controllers this crate knows how to drive.
const MANAGED_CONTROLLERS: &[&str] = &["cpu", "memory", "pids", "io"];

/// The result of trying to enable controllers for a cgroup's children.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ControllerEnable {
    /// Controllers that are enabled (already were, or were enabled now).
    pub enabled: Vec<String>,
    /// Controllers that could not be enabled, with the kernel's reason.
    pub failed: Vec<(String, String)>,
}

/// A discovered cgroup v2 hierarchy rooted at a writable (or read-only) path.
#[derive(Debug, Clone)]
pub struct CgroupRoot {
    path: PathBuf,
    controllers: Vec<String>,
    subtree_control: Vec<String>,
    writable: bool,
}

impl CgroupRoot {
    /// Opens the conventional `/sys/fs/cgroup` mount.
    pub fn detect() -> Result<Self> {
        Self::at(CGROUP2_MOUNT)
    }

    /// Picks the best writable root for the current user.
    ///
    /// Order: the cgroup v2 mount itself (root), then the systemd user
    /// manager's delegated subtree. Falls back to the read-only mount so the
    /// support matrix can still describe the host.
    pub fn detect_writable() -> Result<Self> {
        let root = Self::detect()?;
        if root.writable {
            return Ok(root);
        }

        let uid = nix::unistd::Uid::effective().as_raw();
        let candidates = [
            format!("/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service"),
            format!("/sys/fs/cgroup/user.slice/user-{uid}.slice"),
        ];
        for candidate in candidates {
            if let Ok(candidate_root) = Self::at(&candidate) {
                if candidate_root.writable {
                    return Ok(candidate_root);
                }
            }
        }

        Ok(root)
    }

    /// Opens an explicit cgroup v2 directory.
    pub fn at(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let controllers_file = path.join("cgroup.controllers");
        if !controllers_file.exists() {
            bail!(
                "{} is not a cgroup v2 directory (missing cgroup.controllers)",
                path.display()
            );
        }

        Ok(Self {
            controllers: read_words(&controllers_file),
            subtree_control: read_words(&path.join("cgroup.subtree_control")),
            writable: is_writable(&path.join("cgroup.subtree_control")),
            path,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn controllers(&self) -> &[String] {
        &self.controllers
    }

    #[must_use]
    pub fn subtree_control(&self) -> &[String] {
        &self.subtree_control
    }

    /// Whether the current process can enable controllers for children here.
    #[must_use]
    pub fn is_writable(&self) -> bool {
        self.writable
    }

    /// Enables controllers for this cgroup's children, one at a time so a
    /// single unavailable controller does not block the others.
    #[must_use]
    pub fn enable_controllers(&self, controllers: &[&str]) -> ControllerEnable {
        let mut result = ControllerEnable::default();
        if !self.writable {
            for controller in controllers {
                result.failed.push((
                    (*controller).to_string(),
                    "cgroup subtree_control is not writable".to_string(),
                ));
            }
            return result;
        }

        let current = read_words(&self.path.join("cgroup.subtree_control"));
        for controller in controllers {
            if current.iter().any(|entry| entry == controller) {
                result.enabled.push((*controller).to_string());
                continue;
            }
            match fs::write(
                self.path.join("cgroup.subtree_control"),
                format!("+{controller}"),
            ) {
                Ok(()) => result.enabled.push((*controller).to_string()),
                Err(err) => result
                    .failed
                    .push(((*controller).to_string(), err.to_string())),
            }
        }
        result
    }
}

/// One managed agent session, backed by a cgroup v2 directory.
#[derive(Debug, Clone)]
pub struct SessionCgroup {
    path: PathBuf,
    session_id: Uuid,
}

impl SessionCgroup {
    /// Creates a new session subtree under `base`.
    ///
    /// Missing controllers are reported by [`Self::apply`] as
    /// [`Enforcement::Unsupported`]; they do not make creation fail. The
    /// session is keyed by UUID, never by PID.
    pub fn create(base: &CgroupRoot, session_id: Uuid) -> Result<Self> {
        let _ = base.enable_controllers(MANAGED_CONTROLLERS);

        let path = base.path().join(format!("{SESSION_PREFIX}{session_id}"));
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("create session cgroup {}", path.display()));
            }
        }

        if !path.join("cgroup.procs").exists() {
            bail!(
                "{} was created but does not look like a cgroup",
                path.display()
            );
        }

        Ok(Self { path, session_id })
    }

    /// Opens an existing cgroup, such as a systemd scope.
    pub fn open(path: impl Into<PathBuf>, session_id: Uuid) -> Result<Self> {
        let path = path.into();
        if !path.join("cgroup.procs").exists() {
            bail!("{} is not a cgroup (missing cgroup.procs)", path.display());
        }
        Ok(Self { path, session_id })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    /// Writes the requested limits and reports what the kernel accepted.
    ///
    /// The limits are validated first, so a request that would be rejected by
    /// the kernel never reaches a cgroup file.
    pub fn apply(&self, limits: &ResourceLimits) -> Result<AppliedLimits> {
        limits
            .validate()
            .map_err(|err| anyhow!("invalid resource limits: {err}"))?;

        Ok(AppliedLimits {
            cpu_weight: limits
                .cpu_weight
                .map(|value| self.apply_plain("cpu.weight", value)),
            cpu_max: limits
                .cpu_max
                .map(|value| self.apply_plain("cpu.max", value)),
            memory_high_bytes: limits
                .memory_high_bytes
                .map(|value| self.apply_max_u64("memory.high", value)),
            memory_max_bytes: limits
                .memory_max_bytes
                .map(|value| self.apply_max_u64("memory.max", value)),
            io_weight: limits
                .io_weight
                .map(|value| self.apply_io_weight("io.weight", value)),
            pids_max: limits
                .pids_max
                .map(|value| self.apply_max_u64("pids.max", value)),
        })
    }

    /// Moves an existing process into this cgroup.
    ///
    /// Requires write access to both the source and destination cgroup on most
    /// systems; production runs this as root, while unprivileged tests should
    /// open a systemd scope instead.
    pub fn attach(&self, pid: i32) -> Result<()> {
        fs::write(self.path.join("cgroup.procs"), pid.to_string())
            .with_context(|| format!("attach pid {pid} to {}", self.path.display()))
    }

    /// Reads a point-in-time snapshot of usage, events and pressure.
    pub fn usage(&self) -> Result<ResourceUsage> {
        let cpu = parse_cpu_stat(&self.read_text("cpu.stat").unwrap_or_default());
        let memory = crate::stats::MemoryUsage {
            current_bytes: self.read_u64("memory.current").unwrap_or(0),
            peak_bytes: self.read_u64("memory.peak"),
            high_bytes: self.read_limit("memory.high"),
            max_bytes: self.read_limit("memory.max"),
            events: parse_memory_events(&self.read_text("memory.events").unwrap_or_default()),
        };
        let pids_usage = crate::stats::PidsUsage {
            current: self.read_u64("pids.current").unwrap_or(0),
            max: self.read_limit("pids.max"),
            max_events: parse_pids_max_events(&self.read_text("pids.events").unwrap_or_default()),
        };
        let io = parse_io_stat(&self.read_text("io.stat").unwrap_or_default());
        let pressure = crate::stats::Pressure {
            cpu: parse_pressure(&self.read_text("cpu.pressure").unwrap_or_default()),
            memory: parse_pressure(&self.read_text("memory.pressure").unwrap_or_default()),
            io: parse_pressure(&self.read_text("io.pressure").unwrap_or_default()),
        };

        Ok(ResourceUsage {
            schema_version: RESOURCE_USAGE_SCHEMA_VERSION,
            session_id: self.session_id,
            cgroup_path: self.path.display().to_string(),
            populated: self.populated().unwrap_or(false),
            pids: self.pids()?,
            cpu,
            memory,
            pids_usage,
            io,
            pressure,
        })
    }

    /// Every PID in this cgroup and all descendant cgroups.
    pub fn pids(&self) -> Result<Vec<i32>> {
        let mut pids = Vec::new();
        collect_pids(&self.path, &mut pids)?;
        pids.sort_unstable();
        pids.dedup();
        Ok(pids)
    }

    /// Whether the kernel reports `populated 1` for this cgroup.
    pub fn populated(&self) -> Result<bool> {
        let text = fs::read_to_string(self.path.join("cgroup.events"))
            .with_context(|| format!("read {}/cgroup.events", self.path.display()))?;
        Ok(text
            .lines()
            .any(|line| line.split_whitespace().eq(["populated", "1"])))
    }

    /// Kills the whole process tree and waits for it to become empty.
    ///
    /// Prefers `cgroup.kill`; falls back to sending `SIGKILL` to every PID
    /// when the kernel is too old to provide it.
    pub fn kill(&self) -> Result<()> {
        let kill_path = self.path.join("cgroup.kill");
        if kill_path.exists() {
            fs::write(&kill_path, "1").with_context(|| format!("write {}", kill_path.display()))?;
        } else {
            for pid in self.pids()? {
                match kill(Pid::from_raw(pid), Signal::SIGKILL) {
                    Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
                    Err(err) => bail!("failed to kill pid {pid}: {err}"),
                }
            }
        }

        if !self.wait_empty(Duration::from_secs(5))? {
            bail!(
                "cgroup {} is still populated after kill",
                self.path.display()
            );
        }
        Ok(())
    }

    /// Polls until the cgroup is empty or `timeout` elapses.
    ///
    /// A cgroup that no longer exists is treated as empty: systemd may collect
    /// a transient scope as soon as its last process exits.
    pub fn wait_empty(&self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.path.exists() {
                return Ok(true);
            }
            if !self.populated()? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Removes this cgroup and any descendant cgroups.
    pub fn remove(self) -> Result<()> {
        remove_tree(&self.path)
    }

    fn apply_plain<T>(&self, file: &str, requested: T) -> AppliedLimit<T>
    where
        T: Clone + PartialEq + fmt::Display + FromStr,
        T::Err: fmt::Display,
    {
        let path = self.path.join(file);
        match fs::write(&path, requested.to_string()) {
            Ok(()) => match fs::read_to_string(&path) {
                Ok(text) => {
                    let text = text.trim();
                    match text.parse::<T>() {
                        Ok(applied) if applied == requested => AppliedLimit {
                            requested,
                            applied: Some(applied),
                            enforcement: Enforcement::Enforced,
                            note: None,
                        },
                        Ok(applied) => AppliedLimit {
                            requested,
                            applied: Some(applied),
                            enforcement: Enforcement::Observed,
                            note: Some("kernel reports a different value".to_string()),
                        },
                        Err(err) => AppliedLimit {
                            requested,
                            applied: None,
                            enforcement: Enforcement::Observed,
                            note: Some(format!("read back {file} = {text:?}: {err}")),
                        },
                    }
                }
                Err(err) => AppliedLimit {
                    requested,
                    applied: None,
                    enforcement: Enforcement::Observed,
                    note: Some(format!("read back {file} failed: {err}")),
                },
            },
            Err(err) => AppliedLimit {
                requested,
                applied: None,
                enforcement: classify_error(&err),
                note: Some(format!("write {file} failed: {err}")),
            },
        }
    }

    fn apply_max_u64(&self, file: &str, requested: u64) -> AppliedLimit<u64> {
        let path = self.path.join(file);
        match fs::write(&path, requested.to_string()) {
            Ok(()) => match fs::read_to_string(&path) {
                Ok(text) => {
                    let text = text.trim();
                    if text == "max" {
                        return AppliedLimit {
                            requested,
                            applied: None,
                            enforcement: Enforcement::Observed,
                            note: Some(format!("{file} reads back as max (unlimited)")),
                        };
                    }
                    match text.parse::<u64>() {
                        Ok(applied) if applied == requested => AppliedLimit {
                            requested,
                            applied: Some(applied),
                            enforcement: Enforcement::Enforced,
                            note: None,
                        },
                        Ok(applied) => AppliedLimit {
                            requested,
                            applied: Some(applied),
                            enforcement: Enforcement::Observed,
                            note: Some("kernel reports a different value".to_string()),
                        },
                        Err(err) => AppliedLimit {
                            requested,
                            applied: None,
                            enforcement: Enforcement::Observed,
                            note: Some(format!("read back {file} = {text:?}: {err}")),
                        },
                    }
                }
                Err(err) => AppliedLimit {
                    requested,
                    applied: None,
                    enforcement: Enforcement::Observed,
                    note: Some(format!("read back {file} failed: {err}")),
                },
            },
            Err(err) => AppliedLimit {
                requested,
                applied: None,
                enforcement: classify_error(&err),
                note: Some(format!("write {file} failed: {err}")),
            },
        }
    }

    fn apply_io_weight(&self, file: &str, requested: u64) -> AppliedLimit<u64> {
        let path = self.path.join(file);
        match fs::write(&path, format!("default {requested}")) {
            Ok(()) => match fs::read_to_string(&path) {
                Ok(text) => {
                    let text = text.trim();
                    let applied = text
                        .split_whitespace()
                        .last()
                        .and_then(|raw| raw.parse().ok());
                    match applied {
                        Some(applied) if applied == requested => AppliedLimit {
                            requested,
                            applied: Some(applied),
                            enforcement: Enforcement::Enforced,
                            note: None,
                        },
                        Some(applied) => AppliedLimit {
                            requested,
                            applied: Some(applied),
                            enforcement: Enforcement::Observed,
                            note: Some("kernel reports a different value".to_string()),
                        },
                        None => AppliedLimit {
                            requested,
                            applied: None,
                            enforcement: Enforcement::Observed,
                            note: Some(format!("read back {file} = {text:?}")),
                        },
                    }
                }
                Err(err) => AppliedLimit {
                    requested,
                    applied: None,
                    enforcement: Enforcement::Observed,
                    note: Some(format!("read back {file} failed: {err}")),
                },
            },
            Err(err) => AppliedLimit {
                requested,
                applied: None,
                enforcement: classify_error(&err),
                note: Some(format!("write {file} failed: {err}")),
            },
        }
    }

    fn read_text(&self, file: &str) -> Option<String> {
        fs::read_to_string(self.path.join(file)).ok()
    }

    fn read_u64(&self, file: &str) -> Option<u64> {
        self.read_text(file)?.trim().parse().ok()
    }

    fn read_limit(&self, file: &str) -> Option<u64> {
        parse_limit(&self.read_text(file)?)
    }
}

fn read_words(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Opens a cgroup control file for writing without changing it.
pub(crate) fn is_writable(path: &Path) -> bool {
    OpenOptions::new().write(true).open(path).is_ok()
}

fn classify_error(err: &std::io::Error) -> Enforcement {
    if err.kind() == ErrorKind::NotFound {
        Enforcement::Unsupported
    } else {
        Enforcement::Observed
    }
}

fn collect_pids(dir: &Path, out: &mut Vec<i32>) -> Result<()> {
    if let Ok(text) = fs::read_to_string(dir.join("cgroup.procs")) {
        for line in text.lines() {
            if let Ok(pid) = line.trim().parse::<i32>() {
                out.push(pid);
            }
        }
    }

    for entry in
        fs::read_dir(dir).with_context(|| format!("read cgroup directory {}", dir.display()))?
    {
        let entry = entry?;
        let child = entry.path();
        if child.join("cgroup.procs").exists() {
            collect_pids(&child, out)?;
        }
    }
    Ok(())
}

fn remove_tree(path: &Path) -> Result<()> {
    for entry in
        fs::read_dir(path).with_context(|| format!("read cgroup directory {}", path.display()))?
    {
        let child = entry?.path();
        if child.join("cgroup.procs").exists() {
            remove_tree(&child)?;
        }
    }
    fs::remove_dir(path).with_context(|| format!("remove cgroup {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::CpuMax;

    #[test]
    fn missing_directory_is_not_a_cgroup() {
        let result = CgroupRoot::at("/definitely/not/a/cgroup");
        assert!(result.is_err());
    }

    #[test]
    fn detect_finds_the_linux_cgroup2_mount() {
        let root = CgroupRoot::detect().expect("cgroup v2 mount");
        assert!(root.path().exists());
        assert!(!root.controllers().is_empty());
    }

    #[test]
    fn cpu_max_read_back_uses_the_kernel_format() {
        let parsed: CpuMax = "50000 100000".parse().expect("cpu.max");
        assert_eq!(parsed, CpuMax::new(50_000, 100_000));
    }
}
