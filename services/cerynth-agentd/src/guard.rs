//! Optional integration with Person 2's existing Guard protocol; no policy fallback.
use crate::{AgentService, process::PreparedLaunch, registry::Registry};
use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{LaunchRequest, Session};
use cerynth_agent_protocols::guard::GuardClient;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};
use uuid::Uuid;

fn accepted(response: &Value) -> bool {
    response["type"] == "ok"
}

impl AgentService {
    pub(crate) async fn bind_guard(
        &self,
        session: &Session,
        launch: &LaunchRequest,
        plan: &mut PreparedLaunch,
    ) -> Result<()> {
        let Some(policy) = &session.spec.runtime.guard else {
            return Ok(());
        };
        let client = self
            .guard_control
            .as_ref()
            .context("Guard is required; configure CERYNTH_GUARD_SOCKET")?;
        ensure!(
            plan.unit.is_some(),
            "Guard launch requires an owned systemd scope"
        );
        self.check_guard_paths(session, client)?;
        self.runtime.lock().await.registry.update(session.id, |s| {
            s.execution.guard_socket = Some(client.socket().into());
            s.execution.guard_cleanup_pending = true;
            Registry::record_event(s, "guard_registration_requested");
            Ok(())
        })?;
        let registration = json!({"op":"register_session","registration":{
            "session_id":session.id,"parent_session_id":session.spec.runtime.parent_id,
            "mode":"managed","workspace":session.spec.workspace,"capabilities":policy.capabilities,
            "harness":"generic_pty","remotes":{}}});
        let response = client.request(registration).await?;
        if !accepted(&response) {
            // Explicit denial of this new UUID grants no authority. Transport
            // failure instead leaves cleanup pending because registration is unknown.
            ensure!(
                response["type"] == "error",
                "invalid Guard registration response"
            );
            self.runtime.lock().await.registry.update(session.id, |s| {
                s.execution.guard_cleanup_pending = false;
                Registry::record_event(s, "guard_registration_denied");
                Ok(())
            })?;
            let code = response["code"]
                .as_str()
                .filter(|code| {
                    code.len() <= 128 && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                })
                .unwrap_or("unknown_reason");
            anyhow::bail!("Guard denied session registration: {code}");
        }
        let program = launch.program.canonicalize()?;
        let program = program.strip_prefix(&session.spec.workspace).map_or_else(
            |_| program.clone(),
            |relative| PathBuf::from("/workspace").join(relative),
        );
        let mut cmd = vec![program.to_string_lossy().into_owned()];
        cmd.extend(launch.args.clone());
        let response = client
            .request(json!({"op":"launch_profile","session_id":session.id,"cmd":cmd}))
            .await?;
        ensure!(
            response["type"] == "launch",
            "Guard denied the launch profile"
        );
        let mut argv: Vec<String> = serde_json::from_value(response["argv"].clone())?;
        ensure!(
            !argv.is_empty()
                && argv.len() <= 512
                && argv.iter().all(|a| a.len() <= 8192 && !a.contains('\0')),
            "invalid Guard launch argv"
        );
        ensure!(
            matches!(argv[0].as_str(), "bwrap" | "/usr/bin/bwrap")
                && argv.iter().any(|a| a == "/run/agent/cerynth-guard-exec"),
            "unsupported Guard launcher"
        );
        argv[0] = "/usr/bin/bwrap".into();
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&argv)?));
        let separator = plan
            .args
            .iter()
            .position(|arg| arg == "--")
            .context("resource wrapper is missing")?;
        plan.args.truncate(separator + 1);
        plan.args.extend(argv);
        self.runtime.lock().await.registry.update(session.id, |s| {
            s.execution.guard_profile_digest = Some(digest);
            Registry::record_event(s, "guard_profile_selected");
            Ok(())
        })?;
        Ok(())
    }

    fn check_guard_paths(&self, session: &Session, client: &GuardClient) -> Result<()> {
        ensure!(
            session.execution.worktree.is_none(),
            "provide the final isolated workspace before Guard registration"
        );
        let mut protected = vec![
            self.directory.canonicalize()?,
            client
                .socket()
                .parent()
                .context("Guard socket directory missing")?
                .canonicalize()?,
        ];
        if let Some(path) = std::env::var_os("CERYNTH_INFERENCE_DIR") {
            protected.push(PathBuf::from(path).canonicalize()?);
        }
        ensure!(
            !protected
                .iter()
                .any(|path| path.starts_with(&session.spec.workspace)),
            "workspace would expose a control directory"
        );
        for capability in &session
            .spec
            .runtime
            .guard
            .as_ref()
            .context("Guard policy missing")?
            .capabilities
        {
            if matches!(capability.kind.as_str(), "fs.read" | "fs.write")
                && capability.target != "workspace"
            {
                let path = PathBuf::from(capability.target.trim_end_matches('*')).canonicalize()?;
                ensure!(
                    !protected
                        .iter()
                        .any(|p| p.starts_with(&path) || path.starts_with(p)),
                    "Guard grants must not expose control directories"
                );
            }
        }
        Ok(())
    }

    /// End leaf authorizations only after their process boundary is empty. A
    /// Guard failure remains visible and blocks archival; it never means success.
    pub(crate) async fn release_guard_sessions(&self) -> Result<()> {
        let _cleanup = self.guard_cleanup_lock.lock().await;
        let mut unavailable = BTreeSet::new();
        loop {
            let sessions = self.runtime.lock().await.registry.list();
            let eligible: Vec<_> = sessions
                .iter()
                .filter(|s| {
                    s.state.is_terminal()
                        && !s.execution.cleanup_pending
                        && s.execution.guard_cleanup_pending
                        && !sessions.iter().any(|child| {
                            child.spec.runtime.parent_id == Some(s.id)
                                && (child.execution.guard_cleanup_pending
                                    || child.execution.cleanup_pending
                                    || !child.state.is_terminal())
                        })
                })
                .cloned()
                .collect();
            let mut progress = false;
            for session in eligible {
                let Some(socket) = session.execution.guard_socket.clone() else {
                    continue;
                };
                if unavailable.contains(&socket) {
                    continue;
                }
                let client = GuardClient::new(socket.clone())?;
                let response = client
                    .request(json!({"op":"end_session","session_id":session.id}))
                    .await;
                let ended = match response {
                    Ok(response) if accepted(&response) => true,
                    Ok(response)
                        if response["type"] == "error" && response["code"] == "session_unknown" =>
                    {
                        // The same code also means a busy parent. Only an empty
                        // Guard registry proves there is no remaining authority.
                        client
                            .request(json!({"op":"status"}))
                            .await
                            .is_ok_and(|s| s["type"] == "status" && s["sessions"] == 0)
                    }
                    _ => false,
                };
                if ended {
                    self.runtime.lock().await.registry.update(session.id, |s| {
                        s.execution.guard_cleanup_pending = false;
                        Registry::record_event(s, "guard_session_ended");
                        Ok(())
                    })?;
                    progress = true;
                } else {
                    unavailable.insert(socket);
                }
            }
            if !progress {
                return Ok(());
            }
        }
    }

    pub(crate) async fn cleanup_session(self: &Arc<Self>, id: Uuid) -> Result<Value> {
        let session = {
            let mut runtime = self.runtime.lock().await;
            let session = runtime.registry.get(id)?;
            ensure!(
                session.state.is_terminal(),
                "stop the session before retrying cleanup"
            );
            crate::orchestration::cancel_tree(&mut runtime, id, false, "parent_cleanup")?;
            session
        };
        if session.execution.cleanup_pending {
            tokio::task::spawn_blocking(move || crate::process::recover(&session)).await??;
            self.runtime.lock().await.registry.update(id, |s| {
                s.execution.cleanup_pending = false;
                s.execution.workspace_preparing = false;
                Ok(())
            })?;
        }
        self.release_guard_sessions().await?;
        self.session_records(Some(id)).await
    }
}
