//! Sandbox profiles: a session's capabilities → the exact `bwrap` command
//! line, plus the in-sandbox `cerynth-guard-exec` arguments that add Landlock
//! and seccomp on top.
//!
//! Layering (see docs/security/README.md):
//!   L1 namespaces + bind mounts (bwrap) — only granted paths exist at all
//!   L2 cleared environment with an allowlist
//!   L3 private network namespace; egress only through the Guard proxy socket
//!   L4 Landlock, set by guard-exec on itself before exec'ing the agent
//!   L5 seccomp, set by guard-exec after Landlock

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::capability::{CapabilityKind, CapabilitySet};

/// Where Guard-provided things appear inside every sandbox.
pub const AGENT_RUN_DIR: &str = "/run/agent";
pub const GUARD_EXEC_IN_SANDBOX: &str = "/run/agent/cerynth-guard-exec";
pub const GUARD_SOCKET_IN_SANDBOX: &str = "/run/agent/guard.sock";
pub const EGRESS_SOCKET_IN_SANDBOX: &str = "/run/agent/egress.sock";
/// The in-sandbox loopback port guard-exec forwards to the egress socket.
pub const EGRESS_PORT: u16 = 8099;

const SYSTEM_RO_DIRS: &[&str] = &[
    "/usr", "/bin", "/sbin", "/lib", "/lib64", "/lib32", "/etc", "/opt",
];

/// Agent modes from the threat model. Only `Managed` sessions are confined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMode {
    #[default]
    Managed,
    Integrated,
    Observed,
}

/// Guard's view of a session. The identity and lifecycle are owned by P3's
/// `AgentSession`; Guard only keeps what it needs to confine and decide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuardSession {
    pub session_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<Uuid>,
    #[serde(default)]
    pub mode: SessionMode,
    pub workspace: PathBuf,
    #[serde(default)]
    pub capabilities: CapabilitySet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
}

/// Host-side paths the launcher needs. guardd fills in the sockets; the dev
/// CLI can leave them empty.
#[derive(Debug, Clone, Default)]
pub struct HostPaths {
    pub guard_exec: PathBuf,
    pub guard_socket: Option<PathBuf>,
    pub egress_socket: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxProfile {
    pub workspace: PathBuf,
    pub extra_ro: Vec<PathBuf>,
    pub extra_rw: Vec<PathBuf>,
    pub gpu_devices: Vec<PathBuf>,
    pub egress: bool,
    pub env: Vec<(String, String)>,
}

impl SandboxProfile {
    /// Build a profile from a session's capabilities.
    ///
    /// # Errors
    /// The workspace must be an existing absolute directory, and granted
    /// paths must be absolute (relative grants are ambiguous, so they are
    /// rejected rather than guessed).
    pub fn for_session(session: &GuardSession) -> Result<Self, String> {
        let workspace = session
            .workspace
            .canonicalize()
            .map_err(|e| format!("workspace {}: {e}", session.workspace.display()))?;
        if !workspace.is_dir() {
            return Err(format!(
                "workspace {} is not a directory",
                workspace.display()
            ));
        }
        let caps = &session.capabilities;
        // Mounts and device nodes are installed for the lifetime of the
        // sandbox. They cannot enforce per-action approvals or exceptions to
        // a broader mount. Reject those profiles instead of silently turning
        // an approval or deny into access.
        if caps.0.iter().any(|c| {
            matches!(
                c.kind,
                CapabilityKind::FsRead | CapabilityKind::FsWrite | CapabilityKind::GpuUse
            ) && c.mode != crate::capability::Mode::Allow
        }) {
            return Err("filesystem and GPU mount restrictions requiring deny or approval are not supported by this sandbox profile".into());
        }
        let abs = |kind: CapabilityKind| -> Result<Vec<PathBuf>, String> {
            caps.of_kind(kind)
                .filter(|c| c.target != "workspace")
                .map(|c| {
                    let p = PathBuf::from(c.target.trim_end_matches('*'));
                    if p.is_absolute() {
                        Ok(p)
                    } else {
                        Err(format!(
                            "{} target must be absolute: {}",
                            kind.as_str(),
                            c.target
                        ))
                    }
                })
                .collect()
        };
        let gpu_devices = if caps.of_kind(CapabilityKind::GpuUse).next().is_some() {
            gpu_device_nodes()
        } else {
            Vec::new()
        };
        let egress = caps.of_kind(CapabilityKind::NetEgress).next().is_some()
            || caps.of_kind(CapabilityKind::InferenceUse).next().is_some();
        let mut env = vec![
            ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
            ("HOME".into(), "/home/agent".into()),
            ("USER".into(), "agent".into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("CERYNTH_SANDBOX".into(), "1".into()),
            ("CERYNTH_SESSION_ID".into(), session.session_id.to_string()),
            (
                "CERYNTH_GUARD_SOCKET".into(),
                GUARD_SOCKET_IN_SANDBOX.into(),
            ),
        ];
        if egress {
            let proxy = format!("http://127.0.0.1:{EGRESS_PORT}");
            for k in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
                env.push((k.into(), proxy.clone()));
            }
            env.push(("NO_PROXY".into(), "127.0.0.1,localhost".into()));
            env.push(("CERYNTH_EGRESS".into(), proxy));
        }
        Ok(Self {
            workspace,
            extra_ro: abs(CapabilityKind::FsRead)?,
            extra_rw: abs(CapabilityKind::FsWrite)?,
            gpu_devices,
            egress,
            env,
        })
    }

    /// Paths Landlock should allow read-only inside the sandbox.
    #[must_use]
    pub fn landlock_ro(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = SYSTEM_RO_DIRS
            .iter()
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .collect();
        v.extend(self.extra_ro.iter().cloned());
        v.push(PathBuf::from(AGENT_RUN_DIR));
        v
    }

    /// Paths Landlock should allow read-write inside the sandbox.
    #[must_use]
    pub fn landlock_rw(&self) -> Vec<PathBuf> {
        let mut v = vec![
            PathBuf::from("/workspace"),
            PathBuf::from("/tmp"),
            PathBuf::from("/home/agent"),
            PathBuf::from("/dev"),
            PathBuf::from("/proc"),
        ];
        v.extend(self.extra_rw.iter().cloned());
        v
    }

    /// The full argv: `bwrap … -- /run/agent/cerynth-guard-exec … -- CMD…`.
    #[must_use]
    pub fn command_line(&self, host: &HostPaths, cmd: &[String]) -> Vec<String> {
        let mut a: Vec<String> = vec!["bwrap".into()];
        let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| (*s).to_string()));
        push(&[
            "--unshare-user",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-net",
            "--unshare-cgroup-try",
            "--hostname",
            "cerynth-agent",
            "--die-with-parent",
            "--new-session",
            "--cap-drop",
            "ALL",
            "--clearenv",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--tmpfs",
            "/home",
            "--dir",
            "/home/agent",
            "--tmpfs",
            "/run",
            "--dir",
            AGENT_RUN_DIR,
        ]);
        for (k, v) in &self.env {
            a.extend(["--setenv".into(), k.clone(), v.clone()]);
        }
        for d in SYSTEM_RO_DIRS {
            if Path::new(d).exists() {
                a.extend(["--ro-bind".into(), (*d).into(), (*d).into()]);
            }
        }
        for p in &self.extra_ro {
            a.extend([
                "--ro-bind-try".into(),
                p.display().to_string(),
                p.display().to_string(),
            ]);
        }
        for p in &self.extra_rw {
            a.extend([
                "--bind-try".into(),
                p.display().to_string(),
                p.display().to_string(),
            ]);
        }
        for p in &self.gpu_devices {
            a.extend([
                "--dev-bind-try".into(),
                p.display().to_string(),
                p.display().to_string(),
            ]);
        }
        a.extend([
            "--bind".into(),
            self.workspace.display().to_string(),
            "/workspace".into(),
            "--chdir".into(),
            "/workspace".into(),
        ]);
        a.extend([
            "--ro-bind".into(),
            host.guard_exec.display().to_string(),
            GUARD_EXEC_IN_SANDBOX.into(),
        ]);
        if let Some(s) = &host.guard_socket {
            a.extend([
                "--bind".into(),
                s.display().to_string(),
                GUARD_SOCKET_IN_SANDBOX.into(),
            ]);
        }
        let egress_sock = host.egress_socket.as_ref().filter(|_| self.egress);
        if let Some(s) = egress_sock {
            a.extend([
                "--bind".into(),
                s.display().to_string(),
                EGRESS_SOCKET_IN_SANDBOX.into(),
            ]);
        }
        a.push("--".into());
        a.push(GUARD_EXEC_IN_SANDBOX.into());
        for p in self.landlock_ro() {
            a.extend(["--ro".into(), p.display().to_string()]);
        }
        for p in self.landlock_rw() {
            a.extend(["--rw".into(), p.display().to_string()]);
        }
        if egress_sock.is_some() {
            a.extend([
                "--egress-socket".into(),
                EGRESS_SOCKET_IN_SANDBOX.into(),
                "--egress-port".into(),
                EGRESS_PORT.to_string(),
            ]);
        }
        a.push("--".into());
        a.extend(cmd.iter().cloned());
        a
    }
}

/// GPU device nodes that exist on this host (DRM render nodes and NVIDIA).
#[must_use]
pub fn gpu_device_nodes() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if Path::new("/dev/dri").exists() {
        v.push(PathBuf::from("/dev/dri"));
    }
    if let Ok(rd) = std::fs::read_dir("/dev") {
        let mut nv: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("nvidia"))
            })
            .collect();
        nv.sort();
        v.extend(nv);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Capability, Mode};

    fn session(caps: Vec<Capability>) -> GuardSession {
        GuardSession {
            session_id: Uuid::new_v4(),
            parent_session_id: None,
            mode: SessionMode::Managed,
            workspace: std::env::temp_dir(),
            capabilities: CapabilitySet(caps),
            harness: None,
        }
    }

    #[test]
    fn default_profile_has_no_network_no_gpu_and_no_extra_paths() {
        let p = SandboxProfile::for_session(&session(vec![])).unwrap();
        assert!(
            !p.egress && p.gpu_devices.is_empty() && p.extra_ro.is_empty() && p.extra_rw.is_empty()
        );
        let argv = p.command_line(
            &HostPaths {
                guard_exec: "/x/guard-exec".into(),
                ..Default::default()
            },
            &["true".into()],
        );
        assert!(argv.contains(&"--unshare-net".to_string()));
        assert!(argv.contains(&"--clearenv".to_string()));
        assert!(!argv.iter().any(|a| a.contains("HTTP_PROXY")));
        assert!(
            !argv
                .iter()
                .any(|a| a == "/run/cerynth" || a.starts_with("/var/lib/cerynth"))
        );
    }

    #[test]
    fn grants_become_mounts_and_proxy_env() {
        let p = SandboxProfile::for_session(&session(vec![
            Capability::new(CapabilityKind::FsRead, "/usr/share/dict", Mode::Allow),
            Capability::new(CapabilityKind::FsWrite, "/srv/cache", Mode::Allow),
            Capability::new(
                CapabilityKind::NetEgress,
                "api.anthropic.com:443",
                Mode::Allow,
            ),
        ]))
        .unwrap();
        assert_eq!(p.extra_ro, vec![PathBuf::from("/usr/share/dict")]);
        assert_eq!(p.extra_rw, vec![PathBuf::from("/srv/cache")]);
        assert!(p.egress);
        assert!(
            p.env
                .iter()
                .any(|(k, v)| k == "HTTPS_PROXY" && v.ends_with(":8099"))
        );
    }

    #[test]
    fn mount_restrictions_are_not_silently_widened() {
        for mode in [Mode::Deny, Mode::Approval] {
            for kind in [
                CapabilityKind::FsRead,
                CapabilityKind::FsWrite,
                CapabilityKind::GpuUse,
            ] {
                let s = session(vec![Capability::new(kind, "/tmp", mode)]);
                assert!(SandboxProfile::for_session(&s).is_err());
            }
        }
    }

    #[test]
    fn relative_grants_are_rejected() {
        let err = SandboxProfile::for_session(&session(vec![Capability::new(
            CapabilityKind::FsRead,
            "etc/passwd",
            Mode::Allow,
        )]));
        assert!(err.is_err());
    }
}
