//! Credential vault: secrets live here, outside every sandbox, and are only
//! ever attached to outgoing requests by guardd. Agents get a credential's
//! *name* in their capabilities (`credential.use`), never its value.
//!
//! Layout: one JSON file per credential in the vault directory.
//!
//! ```json
//! { "kind": "http_header", "header": "x-api-key", "value": "…",
//!   "base_url": "https://api.anthropic.com" }
//! { "kind": "git_https", "username": "x-access-token", "token": "…" }
//! ```
//!
//! The directory must be 0700 and each file 0600, owned by guardd's user;
//! otherwise loading fails (a world-readable vault isn't a vault).

use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Credential {
    HttpHeader {
        header: String,
        value: String,
        base_url: String,
    },
    GitHttps {
        username: String,
        token: String,
    },
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print secret values, even in debug output.
        match self {
            Self::HttpHeader {
                header, base_url, ..
            } => write!(f, "HttpHeader({header} → {base_url}, value=<redacted>)"),
            Self::GitHttps { username, .. } => write!(f, "GitHttps({username}, token=<redacted>)"),
        }
    }
}

impl Credential {
    /// Every secret string in this credential, for redaction.
    pub fn secrets(&self) -> Vec<&str> {
        match self {
            Self::HttpHeader { value, .. } => vec![value.as_str()],
            Self::GitHttps { token, .. } => vec![token.as_str()],
        }
    }
}

#[derive(Debug, Default)]
pub struct Vault {
    creds: BTreeMap<String, Credential>,
}

impl Vault {
    /// # Errors
    /// Bad permissions, wrong owner, or unreadable / malformed files.
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        if !dir.exists() {
            return Ok(Self::default());
        }
        let me = nix::unistd::geteuid().as_raw();
        let check = |p: &Path, max_mode: u32| -> anyhow::Result<()> {
            let m = std::fs::metadata(p)?;
            if m.uid() != me {
                anyhow::bail!("{} is owned by uid {}, expected {me}", p.display(), m.uid());
            }
            if m.permissions().mode() & 0o077 != 0 || m.permissions().mode() & 0o777 > max_mode {
                anyhow::bail!(
                    "{} has mode {:o}; must not be accessible to group/others",
                    p.display(),
                    m.permissions().mode() & 0o777
                );
            }
            Ok(())
        };
        check(dir, 0o700)?;
        let mut creds = BTreeMap::new();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            check(&path, 0o600)?;
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let cred: Credential = serde_json::from_slice(&std::fs::read(&path)?)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            creds.insert(name, cred);
        }
        Ok(Self { creds })
    }

    pub fn get(&self, name: &str) -> Option<&Credential> {
        self.creds.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.creds.keys()
    }

    /// Replace every secret value in `text` with `<redacted>`.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for c in self.creds.values() {
            for s in c.secrets() {
                if s.len() >= 4 {
                    out = out.replace(s, "<redacted>");
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str, mode: u32) {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn loads_strict_vault_and_redacts() {
        let d = tempfile::tempdir().unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        write(
            d.path(),
            "anthropic.json",
            r#"{"kind":"http_header","header":"x-api-key","value":"sk-test-123456","base_url":"https://api.anthropic.com"}"#,
            0o600,
        );
        let v = Vault::load(d.path()).unwrap();
        assert!(v.get("anthropic").is_some());
        assert_eq!(v.redact("key=sk-test-123456!"), "key=<redacted>!");
        assert!(!format!("{:?}", v.get("anthropic").unwrap()).contains("sk-test"));
    }

    #[test]
    fn refuses_loose_permissions() {
        let d = tempfile::tempdir().unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Vault::load(d.path()).is_err(), "world-readable directory");
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        write(
            d.path(),
            "x.json",
            r#"{"kind":"git_https","username":"u","token":"tttttt"}"#,
            0o644,
        );
        assert!(
            Vault::load(d.path()).is_err(),
            "world-readable credential file"
        );
    }
}
