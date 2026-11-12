use crate::{
    AgentService,
    registry::{Registry, now_ms},
};
use anyhow::{Result, ensure};
use cerynth_agent_api::{
    AgentMessage, Artifact, ArtifactKind, SessionSpec, SessionState, TaskAssignment, UnifiedEvent,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
};
use uuid::Uuid;

pub(crate) fn cancel_tree(
    runtime: &mut crate::Runtime,
    id: Uuid,
    include_root: bool,
    reason: &str,
) -> Result<()> {
    let sessions = runtime.registry.list();
    let mut ids = vec![id];
    loop {
        let children: Vec<_> = sessions
            .iter()
            .filter(|s| {
                !ids.contains(&s.id)
                    && s.spec
                        .runtime
                        .parent_id
                        .is_some_and(|parent| ids.contains(&parent))
            })
            .map(|s| s.id)
            .collect();
        if children.is_empty() {
            break;
        }
        ids.extend(children);
    }
    for child in ids
        .iter()
        .rev()
        .filter(|child| include_root || **child != id)
    {
        if !runtime.registry.get(*child)?.state.is_terminal() {
            runtime
                .registry
                .transition(*child, SessionState::Cancelled, reason)?;
            if let Some(cancel) = runtime.active.get(child) {
                let _ = cancel.send(true);
            }
            if let Some(control) = runtime.processes.get(child) {
                let _ = control.cancel.send(true);
            }
        }
    }
    Ok(())
}

impl AgentService {
    pub(crate) async fn set_phase(
        &self,
        id: Uuid,
        phase: cerynth_agent_api::WorkloadPhase,
    ) -> Result<Value> {
        let session = self.runtime.lock().await.registry.update(id, |s| {
            ensure!(!s.state.is_terminal(), "session is terminal");
            s.execution.phase = phase;
            Registry::record_event(s, "workload_phase_reported");
            Ok(())
        })?;
        Ok(serde_json::to_value(session)?)
    }
    pub(crate) async fn retry_session(
        &self,
        id: Uuid,
        acknowledge_prior_effects: bool,
    ) -> Result<Value> {
        Ok(serde_json::to_value(
            self.runtime
                .lock()
                .await
                .registry
                .retry(id, acknowledge_prior_effects)?,
        )?)
    }
    pub(crate) async fn session_records(&self, id: Option<Uuid>) -> Result<Value> {
        self.refresh_observed().await?;
        let runtime = self.runtime.lock().await;
        if let Some(id) = id {
            Ok(serde_json::to_value(runtime.registry.get(id)?)?)
        } else {
            Ok(serde_json::to_value(runtime.registry.list())?)
        }
    }
    pub(crate) async fn refresh_observed(&self) -> Result<()> {
        let mut runtime = self.runtime.lock().await;
        for session in runtime.registry.list() {
            if session.execution.mode == cerynth_agent_api::SessionMode::Observed
                && !session.state.is_terminal()
                && session
                    .execution
                    .process
                    .as_ref()
                    .is_none_or(|identity| !crate::process::matching(identity))
            {
                runtime.registry.transition(
                    session.id,
                    SessionState::Failed,
                    "observed_process_exited_status_unknown",
                )?;
            }
        }
        Ok(())
    }
    pub(crate) async fn archive_session(&self, id: Uuid) -> Result<Value> {
        self.runtime.lock().await.registry.archive(id)?;
        Ok(json!({"archived":id,"metadata_preserved":true}))
    }
    pub(crate) async fn observe(&self, id: Uuid, pid: u32) -> Result<Value> {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            fs::metadata(format!("/proc/{pid}"))?.uid() == nix::unistd::getuid().as_raw(),
            "can only observe processes owned by this user"
        );
        let identity = crate::process::identity(pid)?;
        let mut runtime = self.runtime.lock().await;
        ensure!(
            runtime.registry.get(id)?.state == SessionState::Created,
            "session has already started"
        );
        ensure!(
            runtime.registry.get(id)?.spec.runtime.guard.is_none(),
            "observed sessions cannot claim a Guard launch boundary"
        );
        runtime.registry.update(id, |s| {
            s.execution.process = Some(identity);
            s.execution.mode = cerynth_agent_api::SessionMode::Observed;
            s.boundary = cerynth_agent_api::ExecutionBoundary::UnenforcedPrototype;
            Ok(())
        })?;
        runtime
            .registry
            .transition(id, SessionState::Preparing, "observation_preparing")?;
        runtime
            .registry
            .transition(id, SessionState::Starting, "observation_starting")?;
        Ok(serde_json::to_value(runtime.registry.transition(
            id,
            SessionState::Running,
            "observation_started",
        )?)?)
    }
    pub(crate) async fn authorize_sharing(&self, left: Uuid, right: Uuid) -> Result<Value> {
        ensure!(left != right, "sharing requires two distinct sessions");
        let mut runtime = self.runtime.lock().await;
        let sender = runtime.registry.get(left)?;
        let recipient = runtime.registry.get(right)?;
        ensure!(
            sender.spec.runtime.project == recipient.spec.runtime.project
                && !sender.state.is_terminal()
                && !recipient.state.is_terminal()
                && sender.spec.runtime.capabilities.contains("message.share")
                && recipient
                    .spec
                    .runtime
                    .capabilities
                    .contains("message.share"),
            "sharing capability is not granted"
        );
        runtime.registry.update(left, |s| {
            s.spec.runtime.message_peers.insert(right);
            Registry::record_event(s, "owner_sharing_authorized");
            Ok(())
        })?;
        runtime.registry.update(right, |s| {
            s.spec.runtime.message_peers.insert(left);
            Registry::record_event(s, "owner_sharing_authorized");
            Ok(())
        })?;
        Ok(json!({"left":left,"right":right,"sharing":"authorized_by_control_socket_owner"}))
    }
    pub(crate) async fn stop_session(&self, id: Uuid) -> Result<Value> {
        let mut runtime = self.runtime.lock().await;
        ensure!(
            !runtime.registry.get(id)?.state.is_terminal(),
            "session is terminal"
        );
        cancel_tree(&mut runtime, id, true, "user_cancelled")?;
        let result = serde_json::to_value(runtime.registry.get(id)?)?;
        drop(runtime);
        self.release_guard_sessions().await?;
        self.release_model_residency().await?;
        self.release_resources().await?;
        Ok(result)
    }

    pub(crate) async fn delegate(
        &self,
        parent: Uuid,
        mut spec: SessionSpec,
        task: TaskAssignment,
    ) -> Result<Value> {
        task.validate().map_err(anyhow::Error::msg)?;
        let mut runtime = self.runtime.lock().await;
        let sponsor = runtime.registry.get(parent)?;
        ensure!(
            task.dependencies
                .iter()
                .all(|dep| runtime
                    .registry
                    .get(*dep)
                    .is_ok_and(|s| s.spec.runtime.project == sponsor.spec.runtime.project)),
            "invalid task dependency"
        );
        spec.runtime.parent_id = Some(parent);
        spec.runtime.task = Some(task);
        spec.runtime
            .project
            .clone_from(&sponsor.spec.runtime.project);
        Ok(serde_json::to_value(runtime.registry.create(spec)?)?)
    }

    pub(crate) async fn message(
        &self,
        sender: Uuid,
        recipient: Uuid,
        content: String,
    ) -> Result<Value> {
        ensure!(
            !content.is_empty() && content.len() <= 8192,
            "message must contain 1..8192 bytes"
        );
        let mut runtime = self.runtime.lock().await;
        let source = runtime.registry.get(sender)?;
        let target = runtime.registry.get(recipient)?;
        ensure!(
            !source.state.is_terminal() && !target.state.is_terminal(),
            "message endpoint is terminal"
        );
        ensure!(
            source.spec.runtime.project == target.spec.runtime.project
                && source.spec.runtime.message_peers.contains(&recipient)
                && target.spec.runtime.message_peers.contains(&sender),
            "cross-session sharing is not authorized"
        );
        let message = AgentMessage {
            id: Uuid::new_v4(),
            sender,
            recipient,
            timestamp_ms: now_ms(),
            content,
        };
        runtime.registry.update(recipient, |s| {
            ensure!(
                s.execution.messages.len() < 32,
                "recipient message queue is full"
            );
            s.execution.messages.push(message.clone());
            Registry::record_event(s, "agent_message_received");
            Ok(())
        })?;
        Ok(serde_json::to_value(message)?)
    }

    pub(crate) async fn graph(&self, project: &str) -> Result<Value> {
        let sessions: Vec<_> = self
            .runtime
            .lock()
            .await
            .registry
            .list()
            .into_iter()
            .filter(|s| s.spec.runtime.project == project)
            .collect();
        let mut edges: Vec<_> = sessions
            .iter()
            .filter_map(|s| {
                s.spec
                    .runtime
                    .parent_id
                    .map(|parent| json!({"parent":parent,"child":s.id,"kind":"delegation"}))
            })
            .collect();
        for session in &sessions {
            if let Some(task) = &session.spec.runtime.task {
                edges.extend(task.dependencies.iter().map(|dependency|
                    json!({"dependency":dependency,"dependent":session.id,"kind":"dependency"})));
            }
        }
        Ok(json!({"project":project,"nodes":sessions,"edges":edges}))
    }

    pub(crate) async fn events(&self, id: Uuid, after: u64) -> Result<Value> {
        let session = self.runtime.lock().await.registry.get(id)?;
        let events: Vec<_> = session
            .events
            .iter()
            .filter(|e| e.sequence > after)
            .map(|e| UnifiedEvent {
                schema_version: 1,
                event_id: event_id(id, e.sequence),
                trace_id: id,
                session_id: id,
                parent_session_id: session.spec.runtime.parent_id,
                task_id: session.spec.runtime.task.as_ref().map(|t| t.id),
                timestamp_ms: e.timestamp_ms,
                sequence: e.sequence,
                category: if e.reason.starts_with("guard_") || e.reason.starts_with("native_guard_") {
                    "security"
                } else if e.reason.starts_with("resource") || e.reason.starts_with("gpu_") || e.reason.starts_with("model_residency_") || e.reason=="workload_phase_reported" {
                    "resource"
                } else if e.reason.starts_with("harness_tool") {
                    "tool"
                } else if e.reason.starts_with("inference") {
                    "inference"
                } else {
                    "session"
                }
                .into(),
                name: e.reason.clone(),
                attributes: json!({"state":e.state,"harness_report":session.execution.harness_events.iter().find(|event|
                    event.sequence == e.sequence),"source":if e.reason.starts_with("harness_") {"harness_report"} else {"runtime"}}),
            })
            .collect();
        Ok(
            json!({"events":events,"oldest_sequence":session.events.first().map(|e|e.sequence),
            "last_sequence":session.events.last().map(|e|e.sequence)}),
        )
    }

    pub(crate) async fn worktree(&self, id: Uuid) -> Result<Value> {
        // Serialize repository mutation and launch admission. Checkout itself
        // has a strict deadline and a separate namespace/resource boundary.
        let mut runtime = self.runtime.lock().await;
        let session = runtime.registry.get(id)?;
        ensure!(
            session.spec.runtime.guard.is_none(),
            "create the final worktree before creating a Guard session"
        );
        ensure!(
            matches!(
                session.state,
                SessionState::Created | SessionState::Preparing
            ),
            "worktrees must be prepared before start"
        );
        ensure!(
            !session.execution.workspace_preparing,
            "workspace preparation is already pending"
        );
        if let Some(path) = session.execution.worktree {
            return Ok(json!({"path":path,"existing":true}));
        }
        ensure!(
            !runtime.registry.list().iter().any(|other| other.id != id
                && other.state != SessionState::Created
                && !other.state.is_terminal()
                && {
                    let workspace = other
                        .execution
                        .worktree
                        .as_ref()
                        .unwrap_or(&other.spec.workspace);
                    workspace.starts_with(&session.spec.workspace)
                        || session.spec.workspace.starts_with(workspace)
                        || other.spec.runtime.guard.as_ref().is_some_and(|guard| {
                            guard
                                .capabilities
                                .iter()
                                .any(|cap| cap.kind == "fs.write" && cap.target != "workspace")
                        })
                }),
            "source has an active writer; prepare worktrees before launching those sessions"
        );
        let directory = self.directory.join("worktrees");
        fs::create_dir_all(&directory)?;
        let path = directory.join(id.to_string());
        let branch = format!("cerynth/agent-{id}");
        crate::workspace::begin(&mut runtime.registry, &session, &path)?;
        let result = crate::workspace::create(
            &mut runtime.registry,
            &session,
            &path,
            &branch,
            &self.directory,
        )
        .await;
        runtime.registry.update(id, |s| {
            s.execution.workspace_preparing = s.execution.cleanup_pending;
            if result.is_ok() {
                Registry::record_event(s, "workspace_worktree_created");
            } else {
                s.execution.failure_reason = Some("workspace_prepare_failed".into());
            }
            Ok(())
        })?;
        if let Err(error) = result {
            runtime
                .registry
                .transition(id, SessionState::Failed, "workspace_prepare_failed")?;
            return Err(error);
        }
        Ok(json!({"path":path,"branch":branch,"existing":false,"checkout_filters":"disabled"}))
    }

    pub(crate) async fn collect_artifact(
        &self,
        id: Uuid,
        path: PathBuf,
        kind: ArtifactKind,
    ) -> Result<Value> {
        let mut runtime = self.runtime.lock().await;
        let session = runtime.registry.get(id)?;
        let workspace = session
            .execution
            .worktree
            .as_ref()
            .unwrap_or(&session.spec.workspace)
            .canonicalize()?;
        ensure!(
            !path.is_absolute(),
            "artifact paths must be relative to the session workspace"
        );
        let directory = fs::File::open(&workspace)?;
        let descriptor = rustix::fs::openat2(
            &directory,
            &path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH | rustix::fs::ResolveFlags::NO_SYMLINKS,
        )?;
        let file = fs::File::from(descriptor);
        ensure!(
            file.metadata()?.is_file() && file.metadata()?.len() <= 16 * 1024 * 1024,
            "artifact exceeds 16 MiB"
        );
        let artifacts = self.directory.join("artifacts");
        fs::create_dir_all(&artifacts)?;
        let stored = fs::read_dir(&artifacts)?
            .filter_map(Result::ok)
            .filter_map(|e| e.metadata().ok())
            .fold(0_u64, |total, meta| total.saturating_add(meta.len()));
        ensure!(
            stored.saturating_add(file.metadata()?.len()) <= 256 * 1024 * 1024,
            "artifact storage exceeds 256 MiB"
        );
        let mut snapshot = tempfile::NamedTempFile::new_in(&artifacts)?;
        let mut file = file.take(16 * 1024 * 1024 + 1);
        let mut hasher = Sha256::new();
        let mut buffer = [0; 8192];
        let mut size = 0;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
            size += count as u64;
            ensure!(
                size <= 16 * 1024 * 1024,
                "artifact grew beyond the size limit"
            );
            snapshot.write_all(&buffer[..count])?;
        }
        let artifact = Artifact {
            id: Uuid::new_v4(),
            path,
            kind,
            sha256: format!("{:x}", hasher.finalize()),
            size_bytes: size,
        };
        snapshot.as_file().sync_all()?;
        snapshot.persist(artifacts.join(artifact.id.to_string()))?;
        runtime.registry.update(id, |s| {
            ensure!(
                s.execution.artifacts.len() < 32,
                "artifact count limit reached"
            );
            s.execution.artifacts.push(artifact.clone());
            Registry::record_event(s, "artifact_registered");
            Ok(())
        })?;
        Ok(serde_json::to_value(artifact)?)
    }

    pub(crate) async fn diagnostics(&self) -> Result<Value> {
        let runtime = self.runtime.lock().await;
        Ok(
            json!({"api_version":cerynth_agent_api::API_VERSION,"uid":nix::unistd::getuid().as_raw(),
            "sessions":runtime.registry.list().len(),"active_processes":runtime.processes.len(),
            "active_inference":runtime.active.len(),"allow_unisolated":self.allow_unisolated,
            "bubblewrap_installed":std::path::Path::new("/usr/bin/bwrap").is_file(),
            "systemd_run_installed":std::path::Path::new("/usr/bin/systemd-run").is_file(),
            "person1_resource_api":"not_present","person2_guard_api":"not_present"}),
        )
    }
}

fn event_id(session: Uuid, sequence: u64) -> Uuid {
    let mut bytes = *session.as_bytes();
    for (target, source) in bytes[8..].iter_mut().zip(sequence.to_be_bytes()) {
        *target ^= source;
    }
    Uuid::from_bytes(bytes)
}
