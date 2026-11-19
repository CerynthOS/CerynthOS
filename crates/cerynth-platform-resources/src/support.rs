//! Host capability probing for the resource contract.
//!
//! The support matrix answers a question the contract cannot: on *this*
//! machine, which limits can actually be enforced? A controller may be
//! compiled in, mounted, delegated read-only, or absent from a parent's
//! `cgroup.subtree_control` — all of which look different to a caller.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::cgroup::{CGROUP2_MOUNT, CgroupRoot, is_writable};

/// Current schema version of the support matrix.
pub const SUPPORT_MATRIX_SCHEMA_VERSION: u32 = 1;

struct LimitSpec {
    name: &'static str,
    controller: &'static str,
    file: &'static str,
}

const LIMIT_SPECS: &[LimitSpec] = &[
    LimitSpec {
        name: "cpu.weight",
        controller: "cpu",
        file: "cpu.weight",
    },
    LimitSpec {
        name: "cpu.max",
        controller: "cpu",
        file: "cpu.max",
    },
    LimitSpec {
        name: "memory.high",
        controller: "memory",
        file: "memory.high",
    },
    LimitSpec {
        name: "memory.max",
        controller: "memory",
        file: "memory.max",
    },
    LimitSpec {
        name: "io.weight",
        controller: "io",
        file: "io.weight",
    },
    LimitSpec {
        name: "pids.max",
        controller: "pids",
        file: "pids.max",
    },
];

/// Which PSI files exist on this host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PsiSupport {
    pub cpu: bool,
    pub memory: bool,
    pub io: bool,
}

/// systemd availability, used by callers that prefer transient scopes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemdSupport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Whether `systemctl --user` has a running user manager here.
    pub user_scope: bool,
}

/// Support for one limit file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitSupport {
    pub name: String,
    pub controller: String,
    pub file: String,
    pub present: bool,
    pub writable: bool,
    /// `true` only when the file exists, is writable, and its controller is
    /// available to child session cgroups.
    pub enforceable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A snapshot of what this host can enforce.
#[allow(clippy::struct_excessive_bools)] // diagnostic snapshot; booleans are the clearest shape
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportMatrix {
    pub schema_version: u32,
    pub cgroup_v2: bool,
    pub mount: String,
    /// Directory that was actually inspected (a temporary child when possible).
    pub probe_root: String,
    /// Whether the probe used a temporary child cgroup rather than the root.
    pub probed_in_child: bool,
    pub writable: bool,
    pub controllers: Vec<String>,
    pub subtree_control: Vec<String>,
    pub psi: PsiSupport,
    pub cgroup_kill: bool,
    pub memory_peak: bool,
    pub limits: Vec<LimitSupport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub systemd: Option<SystemdSupport>,
}

impl SupportMatrix {
    /// Probes `root`, creating and removing a temporary child cgroup when the
    /// root is writable. Probing a child is important: systemd often makes a
    /// parent's own limit files read-only while its children are writable.
    #[must_use]
    pub fn probe(root: &CgroupRoot) -> Self {
        let probe_dir = if root.is_writable() {
            let dir = root
                .path()
                .join(format!("cerynth-support-probe-{}", unique_suffix()));
            fs::create_dir(&dir).ok().map(|()| dir)
        } else {
            None
        };

        let inspect = probe_dir.as_deref().unwrap_or(root.path());
        let controllers = read_words(&inspect.join("cgroup.controllers"));
        let subtree_control = root.subtree_control().to_vec();

        let limits = LIMIT_SPECS
            .iter()
            .map(|spec| {
                let path = inspect.join(spec.file);
                let present = path.exists();
                let writable = present && is_writable(&path);
                let controller_available = controllers.iter().any(|c| c == spec.controller);
                let child_enabled = subtree_control.iter().any(|c| c == spec.controller);
                let enforceable = present && writable && controller_available;

                let note = if !present {
                    Some(format!("{} is not present on this host", spec.file))
                } else if !controller_available {
                    Some(format!(
                        "{} controller is not available to this cgroup",
                        spec.controller
                    ))
                } else if !writable {
                    Some("file is not writable by the current user".to_string())
                } else if !child_enabled {
                    Some(format!(
                        "{} is not in subtree_control; child sessions cannot enforce it",
                        spec.controller
                    ))
                } else {
                    None
                };

                LimitSupport {
                    name: spec.name.to_string(),
                    controller: spec.controller.to_string(),
                    file: spec.file.to_string(),
                    present,
                    writable,
                    enforceable,
                    note,
                }
            })
            .collect();

        let matrix = Self {
            schema_version: SUPPORT_MATRIX_SCHEMA_VERSION,
            cgroup_v2: root.path().join("cgroup.controllers").exists(),
            mount: CGROUP2_MOUNT.to_string(),
            probe_root: inspect.display().to_string(),
            probed_in_child: probe_dir.is_some(),
            writable: root.is_writable(),
            controllers: root.controllers().to_vec(),
            subtree_control,
            psi: PsiSupport {
                cpu: inspect.join("cpu.pressure").exists(),
                memory: inspect.join("memory.pressure").exists(),
                io: inspect.join("io.pressure").exists(),
            },
            cgroup_kill: inspect.join("cgroup.kill").exists(),
            memory_peak: inspect.join("memory.peak").exists(),
            limits,
            systemd: systemd_support(),
        };

        if let Some(dir) = probe_dir {
            let _ = fs::remove_dir(dir);
        }
        matrix
    }
}

fn read_words(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn systemd_support() -> Option<SystemdSupport> {
    let version = Command::new("systemctl")
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|text| text.lines().next().map(str::to_string));

    let user_scope = Command::new("systemctl")
        .args(["--user", "is-system-running"])
        .output()
        .is_ok_and(|output| output.status.success());

    version.map(|version| SystemdSupport {
        version: Some(version),
        user_scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cgroup_v2_on_linux() {
        let root = CgroupRoot::detect().expect("cgroup v2");
        let matrix = SupportMatrix::probe(&root);
        assert!(matrix.cgroup_v2);
        assert_eq!(matrix.mount, CGROUP2_MOUNT);
        assert!(!matrix.controllers.is_empty());
        assert_eq!(matrix.limits.len(), LIMIT_SPECS.len());
    }

    #[test]
    fn every_limit_has_a_reason_when_it_is_not_enforceable() {
        let root = CgroupRoot::detect().expect("cgroup v2");
        let matrix = SupportMatrix::probe(&root);
        for limit in &matrix.limits {
            if !limit.enforceable {
                assert!(
                    limit.note.is_some(),
                    "{} is not enforceable but has no note",
                    limit.name
                );
            }
        }
    }
}
