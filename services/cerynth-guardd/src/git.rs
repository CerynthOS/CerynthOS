//! Brokered `git push`.
//!
//! The agent's repository is untrusted: its config and hooks can make git
//! run commands (hooks, `core.fsmonitor`, `core.sshCommand`, URL rewrites…).
//! So Guard never pushes *from* the workspace. Instead it:
//!
//! 1. copies exactly the approved commit into a Guard-owned bare mirror
//!    (a fetch, which doesn't run the source repository's hooks);
//! 2. pushes from the mirror to a URL taken from the session registration,
//!    never from the agent's remote config;
//! 3. runs git with system/global config disabled, hooks off, prompts off,
//!    and the credential passed only through the child's environment.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::json;
use tokio::process::Command;

use crate::vault::{Credential, Vault};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushSpec {
    pub remote: String,
    pub branch: String,
    pub commit: String,
    pub force: bool,
}

/// Parse `remote:refs/heads/<branch>` plus `args.commit` / `args.force`.
///
/// # Errors
/// Anything that isn't an unambiguous, safe push request.
pub fn parse(target: &str, args: &serde_json::Value) -> Result<PushSpec, String> {
    let (remote, refname) = target
        .split_once(':')
        .ok_or("target must be <remote>:refs/heads/<branch>")?;
    let branch = refname
        .strip_prefix("refs/heads/")
        .ok_or("only refs/heads/* can be pushed")?;
    let ok_name = |s: &str| {
        !s.is_empty()
            && !s.starts_with(['-', '/', '.'])
            && !s.ends_with(['/', '.'])
            && !s.contains("..")
            && !s.contains("@{")
            && !s.contains("//")
            && !s.ends_with(".lock")
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c))
    };
    if !ok_name(remote) || remote.contains('/') {
        return Err(format!("invalid remote name: {remote}"));
    }
    if !ok_name(branch) {
        return Err(format!("invalid branch name: {branch}"));
    }
    let commit = args
        .get("commit")
        .and_then(|c| c.as_str())
        .ok_or("args.commit (full commit id) is required")?;
    if !(commit.len() == 40 || commit.len() == 64) || !commit.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err("args.commit must be a full 40- or 64-character commit id".into());
    }
    let force = args
        .get("force")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Ok(PushSpec {
        remote: remote.into(),
        branch: branch.into(),
        commit: commit.to_ascii_lowercase(),
        force,
    })
}

/// Which URL schemes the broker will push to.
#[derive(Debug, Clone, Copy)]
pub struct UrlPolicy {
    pub allow_file: bool,
}

fn check_url(url: &str, policy: UrlPolicy) -> Result<(), String> {
    if url.starts_with("https://")
        || (policy.allow_file && (url.starts_with("file://") || url.starts_with('/')))
    {
        Ok(())
    } else {
        Err(format!("remote URL scheme not allowed: {url} (https only)"))
    }
}

fn hermetic(cmd: &mut Command, home: &Path) {
    cmd.env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/false")
        .env("SSH_ASKPASS", "/bin/false")
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

async fn git(home: &Path, args: &[&str], extra_env: &[(String, String)]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    hermetic(&mut cmd, home);
    cmd.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "credential.helper=",
        // A redirect must not send credential-bearing requests to a
        // different origin; callers can register the final URL explicitly.
        "-c",
        "http.followRedirects=false",
    ]);
    cmd.args(args);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let out = cmd.output().await.map_err(|e| format!("git: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

fn basic(user: &str, token: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = format!("{user}:{token}");
    let b = input.as_bytes();
    let mut out = String::new();
    for chunk in b.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Execute an approved push. Returns a JSON result for the decision.
///
/// # Errors
/// Any failure, with secrets redacted from the message.
#[allow(clippy::too_many_arguments)]
pub async fn push(
    spec: &PushSpec,
    url: &str,
    workspace: &Path,
    mirror_root: &Path,
    session: &str,
    credential: Option<&Credential>,
    vault: &Vault,
    policy: UrlPolicy,
) -> Result<serde_json::Value, String> {
    check_url(url, policy)?;
    let mirror: PathBuf = mirror_root.join(format!("{session}.git"));
    let home = mirror_root.join("home");
    std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    let m = mirror.display().to_string();
    if !mirror.exists() {
        git(&home, &["init", "-q", "--bare", &m], &[]).await?;
    }
    // 1. Copy exactly this commit from the untrusted workspace.
    let ws = workspace.display().to_string();
    git(
        &home,
        &[
            "-c",
            "protocol.file.allow=always",
            "-c",
            "uploadpack.allowAnySHA1InWant=true",
            "--git-dir",
            &m,
            "fetch",
            "-q",
            "--no-tags",
            "--no-write-fetch-head",
            &ws,
            &spec.commit,
        ],
        &[],
    )
    .await
    .map_err(|e| {
        format!(
            "could not copy commit {} from the workspace: {e}",
            spec.commit
        )
    })?;
    git(
        &home,
        &[
            "--git-dir",
            &m,
            "cat-file",
            "-e",
            &format!("{}^{{commit}}", spec.commit),
        ],
        &[],
    )
    .await
    .map_err(|_| format!("{} is not a commit", spec.commit))?;

    // 2. Push from the mirror to the registered URL.
    let mut env = Vec::new();
    if let Some(Credential::GitHttps { username, token }) = credential {
        env = vec![
            ("GIT_CONFIG_COUNT".into(), "1".into()),
            ("GIT_CONFIG_KEY_0".into(), "http.extraHeader".into()),
            (
                "GIT_CONFIG_VALUE_0".into(),
                format!("Authorization: Basic {}", basic(username, token)),
            ),
        ];
    }
    let refspec = format!(
        "{}{}:refs/heads/{}",
        if spec.force { "+" } else { "" },
        spec.commit,
        spec.branch
    );
    let file_allow = if policy.allow_file {
        "protocol.file.allow=always"
    } else {
        "protocol.file.allow=never"
    };
    let out = git(
        &home,
        &[
            "-c",
            file_allow,
            "-c",
            "protocol.ext.allow=never",
            "--git-dir",
            &m,
            "push",
            "--porcelain",
            url,
            &refspec,
        ],
        &env,
    )
    .await
    .map_err(|e| vault.redact(&format!("push failed: {e}")))?;
    Ok(
        json!({ "pushed": spec.commit, "branch": spec.branch, "remote": spec.remote, "output": vault.redact(out.trim()) }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_safe_pushes_only() {
        let c = "a".repeat(40);
        let ok = parse("origin:refs/heads/agent/fix-1", &json!({ "commit": c })).unwrap();
        assert_eq!(
            (ok.remote.as_str(), ok.branch.as_str(), ok.force),
            ("origin", "agent/fix-1", false)
        );
        for bad in [
            "origin",
            "origin:refs/tags/v1",
            "origin:refs/heads/../main",
            "-x:refs/heads/a",
            "origin:refs/heads/a.lock",
            "a/b:refs/heads/x",
            "origin:refs/heads/a@{1}",
        ] {
            assert!(parse(bad, &json!({ "commit": c })).is_err(), "{bad}");
        }
        assert!(parse("origin:refs/heads/x", &json!({ "commit": "abc" })).is_err());
        assert!(parse("origin:refs/heads/x", &json!({})).is_err());
    }

    #[test]
    fn https_only_unless_file_allowed() {
        assert!(
            check_url(
                "https://github.com/a/b.git",
                UrlPolicy { allow_file: false }
            )
            .is_ok()
        );
        assert!(check_url("ssh://git@x/y", UrlPolicy { allow_file: false }).is_err());
        assert!(check_url("ext::sh -c x", UrlPolicy { allow_file: true }).is_err());
        assert!(check_url("/tmp/remote.git", UrlPolicy { allow_file: false }).is_err());
        assert!(check_url("/tmp/remote.git", UrlPolicy { allow_file: true }).is_ok());
    }

    #[test]
    fn basic_auth_encoding() {
        assert_eq!(basic("user", "pass"), "dXNlcjpwYXNz");
        assert_eq!(basic("a", "b"), "YTpi");
    }
    #[tokio::test]
    async fn broker_pushes_exact_commit_without_workspace_hooks() {
        use std::process::Command as StdCommand;
        fn run(dir: &Path, args: &[&str]) -> String {
            let out = StdCommand::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        }
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("work");
        let remote = tmp.path().join("remote.git");
        std::fs::create_dir(&workspace).unwrap();
        run(&workspace, &["init", "-q"]);
        run(&workspace, &["config", "user.name", "Test"]);
        run(
            &workspace,
            &["config", "user.email", "test@example.invalid"],
        );
        std::fs::write(workspace.join("file"), "one").unwrap();
        run(&workspace, &["add", "file"]);
        run(&workspace, &["commit", "-qm", "first"]);
        let commit = run(&workspace, &["rev-parse", "HEAD"]);
        run(
            tmp.path(),
            &["init", "--bare", "-q", remote.to_str().unwrap()],
        );
        let marker = tmp.path().join("hook-ran");
        let hook = workspace.join(".git/hooks/pre-push");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let spec = parse("origin:refs/heads/agent/test", &json!({"commit":commit})).unwrap();
        let result = push(
            &spec,
            remote.to_str().unwrap(),
            &workspace,
            &tmp.path().join("mirrors"),
            "test",
            None,
            &Vault::default(),
            UrlPolicy { allow_file: true },
        )
        .await
        .unwrap();
        assert_eq!(result["pushed"], commit);
        assert_eq!(
            run(
                tmp.path(),
                &[
                    "--git-dir",
                    remote.to_str().unwrap(),
                    "rev-parse",
                    "refs/heads/agent/test"
                ]
            ),
            commit
        );
        assert!(!marker.exists());
    }
}
