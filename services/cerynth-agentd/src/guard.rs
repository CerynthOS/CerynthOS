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

fn profile_arguments(response: &Value) -> Result<Vec<String>> {
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
    Ok(argv)
}

fn native_command(plan: &PreparedLaunch, command: Vec<String>) -> Vec<String> {
    let mut configured = vec!["/usr/bin/env".into()];
    for (key, value) in &plan.environment {
        if matches!(
            key.as_str(),
            "OPENCODE_CONFIG_CONTENT"
                | "OPENCODE_DISABLE_AUTOUPDATE"
                | "OPENCODE_DISABLE_MODELS_FETCH"
                | "DO_NOT_TRACK"
        ) {
            configured.push(format!("{key}={value}"));
        }
    }
    configured.extend(command);
    configured
}

impl AgentService {
    pub(crate) fn check_control_workspace(&self, workspace: &std::path::Path) -> Result<()> {
        let mut protected = vec![self.directory.canonicalize()?];
        protected.push(crate::effects::root());
        if let Some(control) = &self.inference_control {
            protected.push(control.directory().canonicalize()?);
        }
        if let Some(control) = &self.guard_control {
            protected.push(
                control
                    .socket()
                    .parent()
                    .context("Guard control parent missing")?
                    .canonicalize()?,
            );
        }
        let native = crate::native_guard::runtime_root();
        if native.exists() {
            protected.push(native.canonicalize()?);
        }
        ensure!(
            !protected.iter().any(|path| path.starts_with(workspace)),
            "workspace would expose a management control directory"
        );
        Ok(())
    }
    pub(crate) async fn bind_guard(
        &self,
        session: &Session,
        launch: &LaunchRequest,
        plan: &mut PreparedLaunch,
    ) -> Result<()> {
        let Some(policy) = &session.spec.runtime.guard else {
            return Ok(());
        };
        ensure!(
            !policy.capabilities.iter().any(|cap| cap.kind == "gpu.use")
                || session.execution.gpu_job_id.is_some(),
            "GPU device profiles require an admitted supervised job"
        );
        let client = if policy.native_inference {
            let current = self.runtime.lock().await.registry.get(session.id)?;
            GuardClient::new(
                current
                    .execution
                    .guard_socket
                    .context("native Guard gateway was not prepared")?,
            )?
        } else {
            self.guard_control
                .clone()
                .context("Guard is required; configure CERYNTH_GUARD_SOCKET")?
        };
        ensure!(
            plan.unit.is_some(),
            "Guard launch requires an owned systemd scope"
        );
        self.check_guard_paths(session, &client)?;
        self.runtime.lock().await.registry.update(session.id, |s| {
            s.execution.guard_socket = Some(client.socket().into());
            s.execution.guard_cleanup_pending = true;
            Registry::record_event(s, "guard_registration_requested");
            Ok(())
        })?;
        let registration = json!({"op":"register_session","registration":{
            "session_id":session.id,"parent_session_id":session.spec.runtime.parent_id,
            "mode":"managed","workspace":session.spec.workspace,"capabilities":policy.capabilities,
            "harness":session.spec.harness,"remotes":policy.remotes}});
        let response = client.request(registration).await?;
        if !accepted(&response) {
            // Explicit denial of this new UUID grants no authority. Transport
            // failure instead leaves cleanup pending because registration is unknown.
            ensure!(
                response["type"] == "error",
                "invalid Guard registration response"
            );
            self.runtime.lock().await.registry.update(session.id, |s| {
                s.execution.guard_cleanup_pending = s.execution.native_guard_unit.is_some();
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
        self.authorize_gpu(session, launch, &client).await?;
        let program = launch.program.canonicalize()?;
        let program = program.strip_prefix(&session.spec.workspace).map_or_else(
            |_| program.clone(),
            |relative| PathBuf::from("/workspace").join(relative),
        );
        let mut cmd = vec![program.to_string_lossy().into_owned()];
        cmd.extend(launch.args.clone());
        if policy.native_inference {
            cmd = native_command(plan, cmd);
        }
        let response = client
            .request(json!({"op":"launch_profile","session_id":session.id,"cmd":cmd}))
            .await?;
        let mut argv = profile_arguments(&response)?;
        self.journal_guard_socket(session.id, &client, &mut argv)
            .await?;
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

    async fn journal_guard_socket(
        &self,
        id: Uuid,
        client: &GuardClient,
        argv: &mut [String],
    ) -> Result<()> {
        let socket = client
            .socket()
            .parent()
            .context("Guard runtime missing")?
            .join("sessions")
            .join(format!("{id}.sock"));
        let gateway =
            crate::effects::gateway(id, self.effects.clone(), GuardClient::new(socket.clone())?)?;
        let proxy = crate::effects::root().join(format!("{id}.sock"));
        let mut replaced = false;
        for index in 0..argv.len().saturating_sub(2) {
            if matches!(argv[index].as_str(), "--bind" | "--ro-bind")
                && argv[index + 1] == socket.to_string_lossy()
                && argv[index + 2] == "/run/agent/guard.sock"
            {
                argv[index + 1] = proxy.to_string_lossy().into_owned();
                replaced = true;
            }
        }
        ensure!(
            replaced,
            "Guard profile did not expose the expected session broker socket"
        );
        self.effect_gateways.lock().await.insert(id, gateway);
        Ok(())
    }

    async fn authorize_gpu(
        &self,
        session: &Session,
        launch: &LaunchRequest,
        client: &GuardClient,
    ) -> Result<()> {
        let Some(job_id) = session.execution.gpu_job_id else {
            return Ok(());
        };
        let job = self
            .gpu
            .lock()
            .await
            .jobs
            .get(&job_id)
            .cloned()
            .context("GPU lease unavailable")?;
        ensure!(
            job.supervised && job.cleanup_pending && job.request.session_id == session.id,
            "GPU execution lease mismatch"
        );
        let request_id = Uuid::new_v4();
        let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(launch)?));
        let response=client.request(json!({"op":"decide","request":{
            "schema":"cerynth.guard/v1alpha1","request_id":request_id,"session_id":session.id,
            "action":{"kind":"gpu.use","target":job.request.device_id,
                "args":{"job_id":job_id,"memory_bytes":job.request.memory_bytes,"launch_sha256":fingerprint}},
            "context":{"origin":"system","provenance":["cerynth-agentd-owned-gpu-job"]}}})).await?;
        ensure!(
            response["type"] == "decision"
                && response["decision"]["request_id"] == request_id.to_string()
                && response["decision"]["session_id"] == session.id.to_string()
                && response["decision"]["decision"] == "allow"
                && matches!(
                    response["decision"]["enforcement"].as_str(),
                    Some("advisory" | "enforced")
                ),
            "Guard did not authorize the GPU launch"
        );
        self.runtime.lock().await.registry.update(session.id, |s| {
            s.execution.gpu_authorization = Some(cerynth_agent_api::GpuAuthorizationReceipt {
                request_id,
                job_id,
                launch_sha256: fingerprint,
                policy_enforcement: response["decision"]["enforcement"]
                    .as_str()
                    .context("Guard enforcement missing")?
                    .into(),
            });
            Registry::record_event(s, "guard_gpu_authorized");
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
            crate::effects::root(),
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
        let native_root = crate::native_guard::runtime_root();
        if native_root.exists() {
            protected.push(native_root.canonicalize()?);
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
                let private_gateway = session.execution.native_guard_unit.is_some();
                if private_gateway {
                    // This service holds only this root's authority. Stopping
                    // its exact owned scope destroys that authority even if its
                    // control API failed or registration never completed.
                    self.stop_native_guard(&session).await?;
                }
                if ended || private_gateway {
                    self.effect_gateways.lock().await.remove(&session.id);
                    self.runtime.lock().await.registry.update(session.id, |s| {
                        s.execution.guard_cleanup_pending = false;
                        Registry::record_event(
                            s,
                            if ended {
                                "guard_session_ended"
                            } else {
                                "native_guard_gateway_terminated"
                            },
                        );
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
        self.finish_gpu(id, false).await?;
        self.release_model_residency().await?;
        self.release_resources().await?;
        self.session_records(Some(id)).await
    }
}
