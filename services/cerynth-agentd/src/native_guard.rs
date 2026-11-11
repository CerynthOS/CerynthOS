//! Opt-in root-session gateway using the unchanged pinned Guard config/protocol.
//! Each private Guard gets only its session's inference credential, never a provider key.
use crate::{
    AgentService,
    process::{self, PreparedLaunch},
    registry::{self, Registry},
};
use anyhow::{Context, Result, ensure};
use cerynth_agent_api::Session;
use cerynth_agent_protocols::guard::GuardClient;
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    process::Stdio,
    time::Duration,
};
use tokio::process::{Child, Command};
use uuid::Uuid;

pub(crate) struct OwnedGuard {
    child: Child,
    unit: Option<String>,
}
struct SecretFile(PathBuf);
impl Drop for SecretFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
impl Drop for OwnedGuard {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        if let Some(unit) = self.unit.take() {
            let _ = process::stop_scope(&unit);
        }
    }
}

pub(crate) fn runtime_root() -> PathBuf {
    PathBuf::from(format!("/run/user/{}/cerynth-g", nix::unistd::getuid()))
}

fn binary(variable: &str, name: &str) -> Result<PathBuf> {
    let path = std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or(std::env::current_exe()?.with_file_name(name));
    ensure!(path.is_absolute(), "Guard binaries must be absolute");
    path.canonicalize()
        .context("install the pinned Guard binaries or configure their owner overrides")
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

impl AgentService {
    // Credential provisioning, scope journaling and owned-child handoff share one transaction.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn bind_native_guard(
        &self,
        session: &Session,
        token: &str,
        origin: &str,
        plan: &mut PreparedLaunch,
    ) -> Result<()> {
        ensure!(
            session.spec.harness == cerynth_agent_api::HarnessKind::OpenCode,
            "unsupported native Guard harness"
        );
        let mut guards = self.native_guards.lock().await;
        ensure!(
            guards.len() < 2 && !guards.contains_key(&session.id),
            "native Guard admission limit reached (2 root gateways)"
        );
        let supervisor_binary = binary("CERYNTH_GUARDD_BINARY", "cerynth-guardd")?;
        let guard_exec = binary("CERYNTH_GUARD_EXEC_BINARY", "cerynth-guard-exec")?;
        let state = self.directory.join("gateways").join(session.id.to_string());
        registry::private_directory(&state)?;
        let vault = state.join("vault");
        registry::private_directory(&vault)?;
        let root = runtime_root();
        registry::private_directory(&root)?;
        let short = session.id.simple().to_string();
        let runtime = root.join(&short[..12]);
        fs::create_dir(&runtime)?;
        fs::set_permissions(
            &runtime,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )?;
        let control = runtime.join("guard.sock");
        ensure!(
            runtime.as_os_str().len() + "/sessions/".len() + 36 + ".egress.sock".len() < 108,
            "Guard runtime socket path exceeds platform limit"
        );
        let credential = vault.join("inference-session.json");
        let _secret_file = SecretFile(credential.clone());
        write_private(
            &credential,
            &serde_json::to_vec(&json!({"kind":"http_header","header":"authorization",
            "value":format!("Bearer {token}"),"base_url":origin}))?,
        )?;
        let config = state.join("guard.toml");
        let quoted = |path: &std::path::Path| serde_json::to_string(&path.to_string_lossy());
        let text = format!(
            "runtime_dir = {}\nstate_dir = {}\nvault_dir = {}\nguard_exec = {}\n[endpoints.cerynth]\nbase_url = {}\ncredential = \"inference-session\"\n",
            quoted(&runtime)?,
            quoted(&state.join("audit"))?,
            quoted(&vault)?,
            quoted(&guard_exec)?,
            serde_json::to_string(origin)?
        );
        write_private(&config, text.as_bytes())?;
        let unit = process::guard_unit_name(session.id);
        self.runtime.lock().await.registry.update(session.id, |s| {
            s.execution.native_guard_unit = Some(unit.clone());
            s.execution.guard_socket = Some(control.clone());
            s.execution.guard_cleanup_pending = true;
            Registry::record_event(s, "native_guard_gateway_preparing");
            Ok(())
        })?;
        let payload = process::parent_payload(
            vec![
                supervisor_binary.to_string_lossy().into_owned(),
                "serve".into(),
                config.to_string_lossy().into_owned(),
            ],
            std::process::id(),
        );
        let uid = nix::unistd::getuid();
        let child = Command::new("/usr/bin/systemd-run")
            .args([
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                &format!("--unit={unit}"),
                "--property=MemoryHigh=96M",
                "--property=MemoryMax=128M",
                "--property=TasksMax=128",
                "--property=CPUWeight=50",
                "--property=KillMode=control-group",
                "--",
            ])
            .args(payload)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &state)
            .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path=/run/user/{uid}/bus"),
            )
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut owned = OwnedGuard {
            child,
            unit: Some(unit),
        };
        let client = GuardClient::new(control)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            if client
                .request(json!({"op":"status"}))
                .await
                .is_ok_and(|s| s["type"] == "status" && s["sessions"] == 0)
            {
                break;
            }
            ensure!(
                owned.child.try_wait()?.is_none(),
                "native Guard gateway exited before readiness"
            );
            ensure!(
                tokio::time::Instant::now() < deadline,
                "native Guard gateway readiness deadline exceeded"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        // Guard loaded the short-lived grant into its private vault. Keep no
        // credential file once ready; startup recovery also scrubs interrupted provisioning.
        fs::remove_file(credential)?;
        let environment = plan
            .environment
            .iter_mut()
            .find(|(key, _)| key == "OPENCODE_CONFIG_CONTENT")
            .context("OpenCode config missing")?;
        let mut config: serde_json::Value = serde_json::from_str(&environment.1)?;
        config["provider"]["ollama"]["options"]["baseURL"] =
            json!("http://127.0.0.1:8099/cerynth/v1");
        config["provider"]["ollama"]["options"]["apiKey"] = json!("guard-owned");
        environment.1 = config.to_string();
        self.runtime.lock().await.registry.update(session.id, |s| {
            Registry::record_event(s, "native_guard_gateway_ready");
            Ok(())
        })?;
        guards.insert(session.id, owned);
        Ok(())
    }

    pub(crate) async fn stop_native_guard(&self, session: &Session) -> Result<()> {
        let Some(unit) = session.execution.native_guard_unit.clone() else {
            return Ok(());
        };
        ensure!(
            unit == process::guard_unit_name(session.id),
            "invalid native Guard ownership record"
        );
        tokio::task::spawn_blocking(move || process::stop_scope(&unit)).await??;
        if let Some(mut owned) = self.native_guards.lock().await.remove(&session.id) {
            owned.unit = None;
            let _ = tokio::time::timeout(Duration::from_secs(2), owned.child.wait()).await;
        }
        self.scrub_native_guard_secret(session.id)?;
        Ok(())
    }

    fn scrub_native_guard_secret(&self, id: Uuid) -> Result<()> {
        let path = self
            .directory
            .join("gateways")
            .join(id.to_string())
            .join("vault/inference-session.json");
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub(crate) async fn recover_native_guard_secrets(&self) -> Result<()> {
        for session in self.runtime.lock().await.registry.list() {
            if session.execution.native_guard_unit.is_some()
                || session
                    .spec
                    .runtime
                    .guard
                    .as_ref()
                    .is_some_and(|policy| policy.native_inference)
            {
                self.scrub_native_guard_secret(session.id)?;
            }
        }
        Ok(())
    }
}
