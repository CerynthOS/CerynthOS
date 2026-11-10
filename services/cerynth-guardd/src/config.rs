//! guardd configuration (`/etc/cerynth/guard/guardd.toml`), validated up
//! front. Anything invalid stops startup: Guard never runs with a policy or
//! vault it couldn't load.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cerynth_security::audit::AuditLog;
use cerynth_security::policy::PolicyEngine;
use serde::Deserialize;

use crate::state::{Config, Guard};
use crate::vault::Vault;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardConfig {
    #[serde(default)]
    pub endpoints: std::collections::BTreeMap<String, crate::egress::Endpoint>,
    /// Session sockets live here (`/run/cerynth/guard`).
    #[serde(default = "d_runtime")]
    pub runtime_dir: PathBuf,
    /// Audit log and git mirrors (`/var/lib/cerynth/guard`).
    #[serde(default = "d_state")]
    pub state_dir: PathBuf,
    /// Cedar guardrails. Unset: the built-in defaults.
    #[serde(default)]
    pub policy_file: Option<PathBuf>,
    /// Credential directory. Unset: `$CREDENTIALS_DIRECTORY` (systemd-creds),
    /// else no credentials.
    #[serde(default)]
    pub vault_dir: Option<PathBuf>,
    #[serde(default = "d_exec")]
    pub guard_exec: PathBuf,
    #[serde(default = "d_ttl")]
    pub approval_ttl_secs: u64,
    /// Allow pushes to local paths / file:// (tests and air-gapped setups).
    #[serde(default)]
    pub allow_file_remotes: bool,
}

fn d_runtime() -> PathBuf {
    "/run/cerynth/guard".into()
}
fn d_state() -> PathBuf {
    "/var/lib/cerynth/guard".into()
}
fn d_exec() -> PathBuf {
    "/usr/lib/cerynth/cerynth-guard-exec".into()
}
fn d_ttl() -> u64 {
    600
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            endpoints: Default::default(),
            runtime_dir: d_runtime(),
            state_dir: d_state(),
            policy_file: None,
            vault_dir: None,
            guard_exec: d_exec(),
            approval_ttl_secs: d_ttl(),
            allow_file_remotes: false,
        }
    }
}

impl GuardConfig {
    /// # Errors
    /// Unreadable file, unknown keys, or invalid values.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let cfg: Self =
            toml::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// # Errors
    /// The first invalid setting.
    pub fn validate(&self) -> anyhow::Result<()> {
        crate::egress::validate(&self.endpoints)?;
        for (name, p) in [
            ("runtime_dir", &self.runtime_dir),
            ("state_dir", &self.state_dir),
            ("guard_exec", &self.guard_exec),
        ] {
            if !p.is_absolute() {
                anyhow::bail!("{name} must be an absolute path, got {}", p.display());
            }
        }
        for (name, p) in [
            ("policy_file", &self.policy_file),
            ("vault_dir", &self.vault_dir),
        ] {
            if let Some(p) = p {
                if !p.is_absolute() {
                    anyhow::bail!("{name} must be an absolute path, got {}", p.display());
                }
            }
        }
        if !(30..=86_400).contains(&self.approval_ttl_secs) {
            anyhow::bail!(
                "approval_ttl_secs must be between 30 and 86400, got {}",
                self.approval_ttl_secs
            );
        }
        Ok(())
    }

    fn vault_path(&self) -> Option<PathBuf> {
        self.vault_dir
            .clone()
            .or_else(|| std::env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from))
    }

    /// Load policy, vault and audit log and build the Guard state.
    ///
    /// # Errors
    /// Any component that fails to load. Never falls back to a weaker setup.
    pub fn build(&self) -> anyhow::Result<Arc<Guard>> {
        self.validate()?;
        let (policy, source) = match &self.policy_file {
            Some(p) => {
                let text = std::fs::read_to_string(p)
                    .map_err(|e| anyhow::anyhow!("policy {}: {e}", p.display()))?;
                (PolicyEngine::new(&text)?, p.display().to_string())
            }
            None => (
                PolicyEngine::with_defaults()?,
                "built-in defaults".to_string(),
            ),
        };
        let vault = match self.vault_path() {
            Some(dir) => {
                Vault::load(&dir).map_err(|e| anyhow::anyhow!("vault {}: {e}", dir.display()))?
            }
            None => Vault::default(),
        };
        let audit = AuditLog::open(self.state_dir.join("audit.jsonl"))?;
        Guard::new(
            Config {
                endpoints: self.endpoints.clone(),
                runtime_dir: self.runtime_dir.clone(),
                mirror_dir: self.state_dir.join("mirrors"),
                guard_exec: self.guard_exec.clone(),
                approval_ttl: Duration::from_secs(self.approval_ttl_secs),
                allow_file_remotes: self.allow_file_remotes,
                policy_source: source,
            },
            policy,
            vault,
            audit,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_settings() {
        let mut c = GuardConfig::default();
        assert!(c.validate().is_ok());
        c.approval_ttl_secs = 5;
        assert!(c.validate().is_err());
        let c = GuardConfig {
            state_dir: "relative".into(),
            ..GuardConfig::default()
        };
        assert!(c.validate().is_err());
        assert!(
            toml::from_str::<GuardConfig>("unknown_key = 1").is_err(),
            "unknown keys are rejected"
        );
    }

    #[test]
    fn invalid_policy_file_stops_startup() {
        let d = tempfile::tempdir().unwrap();
        let policy = d.path().join("p.cedar");
        std::fs::write(&policy, "permit(principal action resource").unwrap();
        let c = GuardConfig {
            runtime_dir: d.path().join("run"),
            state_dir: d.path().join("state"),
            policy_file: Some(policy),
            ..GuardConfig::default()
        };
        assert!(c.build().is_err());
    }
}
