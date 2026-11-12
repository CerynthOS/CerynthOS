//! Durable owned model-scope identities; recovery never searches or kills by name.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Default)]
pub(crate) struct Journal {
    path: Option<PathBuf>,
    units: BTreeSet<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    units: BTreeSet<String>,
}

fn valid(unit: &str) -> bool {
    unit.strip_prefix("cerynth-model-")
        .and_then(|s| s.strip_suffix(".scope"))
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .is_some_and(|id| format!("cerynth-model-{id}.scope") == unit)
}

impl Journal {
    pub(crate) fn units(&self) -> &BTreeSet<String> {
        &self.units
    }
    pub(crate) fn persistent(directory: &Path) -> Result<Self> {
        let path = directory.join("backend-scopes.json");
        if path.exists() {
            let meta = std::fs::symlink_metadata(&path)?;
            ensure!(
                meta.is_file()
                    && meta.uid() == nix::unistd::getuid().as_raw()
                    && meta.mode() & 0o777 == 0o600
                    && meta.len() <= 16 * 1024,
                "invalid backend scope journal"
            );
            let snapshot: Snapshot = serde_json::from_slice(&std::fs::read(&path)?)?;
            ensure!(
                snapshot.version == 1
                    && snapshot.units.len() <= 32
                    && snapshot.units.iter().all(|u| valid(u)),
                "invalid owned backend identities"
            );
            for unit in &snapshot.units {
                stop(unit).context("previous model scope cleanup is incomplete")?;
            }
        }
        let journal = Self {
            path: Some(path),
            units: BTreeSet::new(),
        };
        journal.persist()?;
        Ok(journal)
    }
    fn persist(&self) -> Result<()> {
        let path = self
            .path
            .as_ref()
            .context("owned model launch requires a persistent coordinator directory")?;
        let parent = path.parent().context("scope journal directory missing")?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(&serde_json::to_vec(&Snapshot {
            version: 1,
            units: self.units.clone(),
        })?)?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
    pub(crate) fn reserve(&mut self) -> Result<String> {
        ensure!(
            self.units.len() < 32,
            "backend scope journal full; reconcile previous cleanup first"
        );
        let unit = format!("cerynth-model-{}.scope", uuid::Uuid::new_v4());
        self.units.insert(unit.clone());
        self.persist()?;
        Ok(unit)
    }
    pub(crate) fn release(&mut self, unit: &str) -> Result<()> {
        stop(unit)?;
        self.units.remove(unit);
        self.persist()
    }
}

fn systemctl(arguments: &[&str]) -> Result<std::process::Output> {
    let uid = nix::unistd::getuid();
    std::process::Command::new("/usr/bin/timeout")
        .args(["--signal=KILL", "8", "/usr/bin/systemctl", "--user"])
        .args(arguments)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path=/run/user/{uid}/bus"),
        )
        .output()
        .context("user systemd model scope control unavailable")
}

pub(crate) fn stop(unit: &str) -> Result<()> {
    ensure!(valid(unit), "refusing unowned model scope");
    let _ = systemctl(&["stop", unit])?;
    let output = systemctl(&[
        "show",
        unit,
        "--property=LoadState",
        "--property=ControlGroup",
    ])?;
    let text = String::from_utf8(output.stdout)?;
    if text.lines().any(|s| s == "LoadState=not-found") {
        return Ok(());
    }
    ensure!(output.status.success(), "cannot inspect owned model scope");
    let group = text
        .lines()
        .find_map(|s| s.strip_prefix("ControlGroup="))
        .context("model scope cgroup missing")?;
    if group.is_empty() {
        return Ok(());
    }
    ensure!(
        group.starts_with('/') && !group.split('/').any(|p| p == ".." || p == "."),
        "invalid model scope cgroup"
    );
    let events = Path::new("/sys/fs/cgroup")
        .join(group.trim_start_matches('/'))
        .join("cgroup.events");
    match std::fs::read_to_string(events) {
        Ok(text) => ensure!(
            text.lines().any(|s| s == "populated 0"),
            "model scope still has processes"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub(crate) fn payload(unit: &str, binary: &Path) -> Vec<String> {
    vec![
        "--user".into(),
        "--scope".into(),
        "--quiet".into(),
        "--collect".into(),
        format!("--unit={unit}"),
        "--property=MemoryHigh=6G".into(),
        "--property=MemoryMax=8G".into(),
        "--property=TasksMax=256".into(),
        "--property=KillMode=control-group".into(),
        "--".into(),
        "/usr/bin/setpriv".into(),
        "--pdeathsig".into(),
        "KILL".into(),
        "/usr/bin/sh".into(),
        "-c".into(),
        "[ \"$PPID\" = \"$1\" ] || exit 125; shift; exec \"$@\"".into(),
        "cerynth-model-parent-guard".into(),
        std::process::id().to_string(),
        binary.to_string_lossy().into_owned(),
        "serve".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_rejects_foreign_scope_before_any_system_action() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("backend-scopes.json");
        std::fs::write(&path, br#"{"version":1,"units":["user.slice"]}"#)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        assert!(Journal::persistent(directory.path()).is_err());
        assert!(!valid(
            "cerynth-agent-00000000-0000-0000-0000-000000000000.scope"
        ));
        assert!(valid(&format!(
            "cerynth-model-{}.scope",
            uuid::Uuid::new_v4()
        )));
        Ok(())
    }
}
