//! Optional owner-invoked SSH prototype. Remote sessions remain owned by remote agentd.
use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{
    API_VERSION, MAX_FRAME_BYTES, MAX_RESPONSE_BYTES, Operation, Request, Response,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    net::IpAddr,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteConfig {
    pub host: IpAddr,
    pub port: u16,
    pub user: String,
    pub identity_file: PathBuf,
    pub known_hosts_file: PathBuf,
    pub agent_binary: PathBuf,
    pub state_directory: PathBuf,
    pub remote_consent: bool,
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

impl RemoteConfig {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.remote_consent
                && self.port > 0
                && !self.host.is_unspecified()
                && !self.host.is_multicast(),
            "remote execution needs explicit consent and an exact destination"
        );
        ensure!(
            !self.user.is_empty()
                && self.user.len() <= 64
                && self
                    .user
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')),
            "invalid remote user"
        );
        for path in [
            &self.agent_binary,
            &self.state_directory,
            &self.identity_file,
            &self.known_hosts_file,
        ] {
            ensure!(
                path.is_absolute()
                    && path
                        .to_str()
                        .is_some_and(|p| p.len() <= 4096 && !p.chars().any(char::is_control)),
                "invalid remote configuration path"
            );
        }
        for path in [&self.identity_file, &self.known_hosts_file] {
            // OpenSSH expands percent tokens and parses whitespace in -o paths.
            ensure!(
                path.to_str()
                    .is_some_and(|p| !p.contains('%') && !p.chars().any(char::is_whitespace)),
                "SSH credential paths must not contain whitespace or percent expansions"
            );
            let metadata = std::fs::symlink_metadata(path)?;
            ensure!(
                metadata.is_file()
                    && metadata.uid() == nix::unistd::getuid().as_raw()
                    && metadata.mode() & 0o022 == 0
                    && metadata.len() <= 65536,
                "SSH key/pin file is not owner-controlled"
            );
            if path == &self.identity_file {
                ensure!(
                    metadata.mode() & (nix::libc::S_IRWXG | nix::libc::S_IRWXO) == 0,
                    "SSH private key must have mode 0600 or stricter"
                );
            }
        }
        Ok(())
    }
}

fn ssh_command(config: &RemoteConfig) -> Result<Command> {
    let remote_command = format!(
        "exec {} --directory {} relay",
        shell_quote(config.agent_binary.to_str().context("invalid binary")?),
        shell_quote(
            config
                .state_directory
                .to_str()
                .context("invalid directory")?
        )
    );
    let mut command = Command::new("/usr/bin/ssh");
    command
        .args([
            "-T",
            "-F",
            "/dev/null",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "IdentityAgent=none",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "PermitLocalCommand=no",
            "-o",
            "ProxyCommand=none",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
            "VerifyHostKeyDNS=no",
            "-o",
            "UpdateHostKeys=no",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "LogLevel=ERROR",
        ])
        .arg("-o")
        .arg(format!(
            "UserKnownHostsFile={}",
            config.known_hosts_file.display()
        ))
        .arg("-i")
        .arg(&config.identity_file)
        .arg("-p")
        .arg(config.port.to_string())
        .arg("-l")
        .arg(&config.user)
        .arg("--")
        .arg(config.host.to_string())
        .arg(remote_command)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    Ok(command)
}

/// One explicit operation over a pinned SSH connection, with no ambient auth/config.
/// # Errors
/// Returns an error on invalid configuration, host-key mismatch, timeout or
/// transport failure. A transport error does not prove the operation did not run.
pub async fn request(config: &RemoteConfig, operation: Operation) -> Result<Value> {
    config.validate()?;
    let mut bytes = serde_json::to_vec(&Request {
        version: API_VERSION,
        operation,
    })?;
    ensure!(
        bytes.len() < MAX_FRAME_BYTES,
        "remote request exceeds limit"
    );
    bytes.push(b'\n');
    let mut child = ssh_command(config)?
        .spawn()
        .context("SSH client unavailable")?;
    let result = tokio::time::timeout(Duration::from_secs(200), async {
        let mut stdin = child.stdin.take().context("SSH stdin unavailable")?;
        stdin.write_all(&bytes).await?;
        stdin.shutdown().await?;
        drop(stdin);
        let mut response = Vec::new();
        child
            .stdout
            .take()
            .context("SSH stdout unavailable")?
            .take(MAX_RESPONSE_BYTES as u64 + 1)
            .read_to_end(&mut response)
            .await?;
        ensure!(
            response.len() <= MAX_RESPONSE_BYTES,
            "remote response exceeds limit"
        );
        ensure!(
            child.wait().await?.success(),
            "SSH transport failed; operation completion may be unknown"
        );
        let response: Response =
            serde_json::from_slice(&response).context("invalid remote response")?;
        ensure!(
            response.version == API_VERSION,
            "remote API version mismatch"
        );
        response.result.map_err(anyhow::Error::msg)
    })
    .await;
    let result = result.unwrap_or_else(|_| {
        Err(anyhow::anyhow!(
            "SSH request deadline exceeded; query the operation before retrying"
        ))
    });
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}

/// Fixed remote entry point: relay one versioned request into the owner's Unix API.
/// # Errors
/// Rejects oversized/incomplete input, unsupported versions and IPC failures.
pub async fn relay(directory: &Path, input: impl AsyncRead + Unpin) -> Result<Response> {
    let mut bytes = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        input
            .take(MAX_FRAME_BYTES as u64 + 1)
            .read_to_end(&mut bytes),
    )
    .await??;
    ensure!(
        bytes.len() <= MAX_FRAME_BYTES && bytes.last() == Some(&b'\n'),
        "invalid relay frame"
    );
    let request: Request = serde_json::from_slice(&bytes)?;
    ensure!(
        request.version == API_VERSION,
        "unsupported relay API version"
    );
    Ok(Response {
        version: API_VERSION,
        result: crate::call(directory, request.operation)
            .await
            .map_err(|e| e.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn relay_rejects_oversized_and_future_version_frames_before_touching_state() -> Result<()>
    {
        let directory = tempfile::tempdir()?;
        let absent = directory.path().join("not-created");
        let future = b"{\"version\":999,\"operation\":{\"type\":\"list\"}}\n";
        assert!(relay(&absent, &future[..]).await.is_err());
        let mut huge = vec![b' '; MAX_FRAME_BYTES + 1];
        huge.push(b'\n');
        assert!(relay(&absent, &huge[..]).await.is_err());
        assert!(!absent.exists());
        Ok(())
    }
    #[test]
    fn remote_command_paths_cannot_execute_shell_substitutions() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let marker = directory.path().join("should-not-exist");
        let value = format!("' $(touch {}) ; echo substituted", marker.display());
        let output = std::process::Command::new("/usr/bin/sh")
            .args(["-c", &format!("printf '%s' {}", shell_quote(&value))])
            .output()?;
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout)?, value);
        assert!(!marker.exists());
        Ok(())
    }
}
