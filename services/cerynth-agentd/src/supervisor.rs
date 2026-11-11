use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{
    ExecutionBoundary, IsolationPolicy, LaunchRequest, OperationRecord, OperationStatus,
    SessionMode, SessionState, WorkloadPhase,
};
use nix::sys::signal::Signal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, watch},
};
use uuid::Uuid;

use crate::{AgentService, process, registry::Registry};

pub(crate) struct ProcessControl {
    pub input: mpsc::Sender<Vec<u8>>,
    pub cancel: watch::Sender<bool>,
}

const MAX_OUTPUT: usize = 1024 * 1024;

// The process exists before state persistence and IO handoff can finish. An error
// in that interval must still tear down the exact owned process generation.
struct LaunchGuard(Option<cerynth_agent_api::Session>);

impl Drop for LaunchGuard {
    fn drop(&mut self) {
        if let Some(session) = self.0.take() {
            let _ = process::recover(&session);
        }
    }
}

async fn copy_output(mut reader: impl AsyncRead + Unpin, send: mpsc::Sender<Vec<u8>>) {
    let mut bytes = [0; 4096];
    loop {
        match reader.read(&mut bytes).await {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                if send.send(bytes[..count].to_vec()).await.is_err() {
                    break;
                }
            }
        }
    }
}

impl AgentService {
    pub(crate) async fn start_generic(
        self: &Arc<Self>,
        id: Uuid,
        operation_id: Uuid,
        launch: LaunchRequest,
    ) -> Result<Value> {
        use cerynth_agent_api::HarnessAdapter;
        let session = self.runtime.lock().await.registry.get(id)?;
        let launch = cerynth_adapter_generic_pty::GenericPtyAdapter
            .prepare_launch(&session, cerynth_agent_api::AdapterInput::Command(launch))
            .await?;
        self.start_process(id, operation_id, launch).await
    }
    // Admission and durable operation recording form one launch transaction.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn start_process(
        self: &Arc<Self>,
        id: Uuid,
        operation_id: Uuid,
        launch: LaunchRequest,
    ) -> Result<Value> {
        while self.workers.lock().await.try_join_next().is_some() {}
        ensure!(
            launch
                .initial_input
                .as_ref()
                .is_none_or(|s| s.len() <= cerynth_agent_api::MAX_PROMPT_BYTES),
            "initial input exceeds limit"
        );
        let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&launch)?));
        let (session, plan) = {
            let mut runtime = self.runtime.lock().await;
            let session = runtime.registry.get(id)?;
            ensure!(
                runtime
                    .registry
                    .operation_owner(operation_id)
                    .is_none_or(|owner| owner == id),
                "operation ID is already bound to another session"
            );
            if let Some(operation) = session
                .execution
                .operations
                .iter()
                .find(|op| op.id == operation_id)
            {
                ensure!(
                    operation.fingerprint == fingerprint,
                    "operation ID reused with different details"
                );
                return Ok(json!({"session":session,"replayed":true,"action_executed":false}));
            }
            ensure!(
                (session.state == SessionState::Created
                    || (session.state == SessionState::Preparing
                        && session.execution.worktree.is_some()))
                    && !session.execution.workspace_preparing,
                "session has already started"
            );
            ensure!(
                session.spec.runtime.parent_id.is_none_or(|parent| runtime
                    .registry
                    .get(parent)
                    .is_ok_and(|p| !p.state.is_terminal())),
                "parent session is terminal or missing"
            );
            ensure!(
                runtime.processes.len() < 16,
                "process admission limit reached"
            );
            ensure!(
                session
                    .spec
                    .runtime
                    .task
                    .as_ref()
                    .is_none_or(|task| task.dependencies.iter().all(|dep| runtime
                        .registry
                        .get(*dep)
                        .is_ok_and(|s| s.state == SessionState::Completed))),
                "task dependencies are incomplete"
            );
            let workspace = session
                .execution
                .worktree
                .as_ref()
                .unwrap_or(&session.spec.workspace);
            if session.spec.runtime.isolation == IsolationPolicy::Required {
                self.check_control_workspace(workspace)?;
            }
            ensure!(
                !runtime.registry.list().iter().any(|other| other.id != id
                    && other.state != SessionState::Created
                    && !other.state.is_terminal()
                    && {
                        let other_workspace = other
                            .execution
                            .worktree
                            .as_ref()
                            .unwrap_or(&other.spec.workspace);
                        other_workspace.starts_with(workspace)
                            || workspace.starts_with(other_workspace)
                    }),
                "workspace already active; create a separate worktree"
            );
            let plan = process::prepare(&session, &launch, self.allow_unisolated)?;
            runtime.registry.update(id, |s| {
                s.execution.operations.push(OperationRecord {
                    id: operation_id,
                    fingerprint,
                    status: OperationStatus::Started,
                });
                s.execution.allocation_id = Some(Uuid::new_v4());
                s.execution.cgroup_unit.clone_from(&plan.unit);
                Ok(())
            })?;
            if session.state == SessionState::Created {
                runtime
                    .registry
                    .transition(id, SessionState::Preparing, "process_preparing")?;
            }
            runtime
                .registry
                .transition(id, SessionState::Starting, "process_starting")?;
            (session, plan)
        };
        let result = async {
            let mut plan = plan;
            self.bind_inference(&session, &mut plan).await?;
            self.bind_guard(&session, &launch, &mut plan).await?;
            let session = self.runtime.lock().await.registry.get(id)?;
            self.spawn_worker(id, operation_id, session, plan, launch)
                .await
        }
        .await;
        if result.is_err() {
            self.revoke_inference(id).await;
            let mut runtime = self.runtime.lock().await;
            runtime.registry.update(id, |s| {
                s.execution.failure_reason = Some("process_spawn_failed".into());
                if let Some(op) = s
                    .execution
                    .operations
                    .iter_mut()
                    .find(|op| op.id == operation_id)
                {
                    // A failed launch acknowledgement does not prove the child
                    // performed no effects before cleanup.
                    op.status = OperationStatus::Unknown;
                }
                Ok(())
            })?;
            if !runtime.registry.get(id)?.state.is_terminal() {
                runtime
                    .registry
                    .transition(id, SessionState::Failed, "process_spawn_failed")?;
            }
            drop(runtime);
            self.release_guard_sessions().await?;
        }
        result
    }

    // Keep process ownership, IO handoff, and guaranteed teardown in one scope.
    #[allow(clippy::too_many_lines)]
    async fn spawn_worker(
        self: &Arc<Self>,
        id: Uuid,
        operation_id: Uuid,
        session: cerynth_agent_api::Session,
        mut plan: process::PreparedLaunch,
        launch: LaunchRequest,
    ) -> Result<Value> {
        process::protect_parent(&mut plan)?;
        let output_dir = self.directory.join("outputs");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&output_dir)?;
        let output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(output_dir.join(id.to_string()))?;
        let workspace = session
            .execution
            .worktree
            .as_ref()
            .unwrap_or(&session.spec.workspace);
        let (output_send, mut output_receive) = mpsc::channel::<Vec<u8>>(32);
        let (input_send, mut input_receive) = mpsc::channel::<Vec<u8>>(8);
        let (cancel, mut cancelled) = watch::channel(false);
        let mut io_tasks = Vec::new();
        let mut runtime = self.runtime.lock().await;
        ensure!(
            runtime.registry.get(id)?.state == SessionState::Starting,
            "launch was cancelled before process creation"
        );
        let (identity, wait): (
            cerynth_agent_api::ProcessIdentity,
            tokio::task::JoinHandle<Result<i32>>,
        ) = if launch.pty {
            let mut pty = cerynth_adapter_generic_pty::spawn(
                &plan.program,
                &plan.args,
                workspace,
                &plan.environment,
            )?;
            let pid = pty.child.process_id().context("PTY child has no PID")?;
            let identity = match process::identity(pid) {
                Ok(identity) => identity,
                Err(error) => {
                    let _ = pty.child.kill();
                    let _ = pty.child.wait();
                    return Err(error);
                }
            };
            let mut reader = pty.reader;
            let reader_send = output_send.clone();
            io_tasks.push(tokio::task::spawn_blocking(move || {
                let mut bytes = [0; 4096];
                while let Ok(count) = reader.read(&mut bytes) {
                    if count == 0 || reader_send.blocking_send(bytes[..count].to_vec()).is_err() {
                        break;
                    }
                }
            }));
            let mut writer = pty.writer;
            io_tasks.push(tokio::task::spawn_blocking(move || {
                while let Some(input) = input_receive.blocking_recv() {
                    if writer.write_all(&input).is_err() || writer.flush().is_err() {
                        break;
                    }
                }
            }));
            let mut child = pty.child;
            let wait = tokio::task::spawn_blocking(move || {
                let _master = pty.master;
                Ok(i32::try_from(child.wait()?.exit_code()).unwrap_or(255))
            });
            (identity, wait)
        } else {
            let mut command = tokio::process::Command::new(&plan.program);
            command
                .args(&plan.args)
                .current_dir(workspace)
                .env_clear()
                .envs(plan.environment.clone())
                .process_group(0)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            let mut child = command.spawn()?;
            let pid = child.id().context("child has no PID")?;
            let identity = match process::identity(pid) {
                Ok(identity) => identity,
                Err(error) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(error);
                }
            };
            let stdout = child.stdout.take().context("stdout unavailable")?;
            let stderr = child.stderr.take().context("stderr unavailable")?;
            io_tasks.push(tokio::spawn(copy_output(stdout, output_send.clone())));
            io_tasks.push(tokio::spawn(copy_output(stderr, output_send.clone())));
            let mut stdin = child.stdin.take().context("stdin unavailable")?;
            let close_initial = launch.initial_input.is_some();
            io_tasks.push(tokio::spawn(async move {
                while let Some(input) = input_receive.recv().await {
                    if stdin.write_all(&input).await.is_err() {
                        break;
                    }
                    if close_initial {
                        let _ = stdin.shutdown().await;
                        break;
                    }
                }
            }));
            (
                identity,
                tokio::spawn(async move { Ok(child.wait().await?.code().unwrap_or(128)) }),
            )
        };
        let mut owned = session.clone();
        owned.execution.process = Some(identity.clone());
        owned.execution.cgroup_unit.clone_from(&plan.unit);
        let recovery = owned.clone();
        let mut guard = LaunchGuard(Some(owned));
        drop(output_send);
        let pid = identity.pid;
        {
            runtime.registry.update(id, |s| {
                s.execution.process = Some(identity.clone());
                s.execution.cleanup_pending = true;
                s.execution.mode = if session.spec.runtime.isolation == IsolationPolicy::Required
                    && plan.unit.is_some()
                {
                    SessionMode::Managed
                } else {
                    SessionMode::Integrated
                };
                s.boundary = if session.spec.runtime.guard.is_some() {
                    ExecutionBoundary::GuardSystemd
                } else if session.spec.runtime.isolation == IsolationPolicy::Required {
                    if plan.unit.is_some() {
                        ExecutionBoundary::BubblewrapSystemd
                    } else {
                        ExecutionBoundary::Bubblewrap
                    }
                } else {
                    ExecutionBoundary::ProcessGroup
                };
                Ok(())
            })?;
            runtime
                .registry
                .transition(id, SessionState::Running, "process_started")?;
            runtime.processes.insert(
                id,
                ProcessControl {
                    input: input_send.clone(),
                    cancel,
                },
            );
        }
        drop(runtime);
        if let Some(input) = launch.initial_input {
            // An early exit can close stdin; the worker must still reap and
            // report that exit rather than abandoning process ownership.
            let _ = input_send.send(input.into_bytes()).await;
        }
        let service = self.clone();
        self.workers.lock().await.spawn(async move {
            let mut output = output;
            let mut size = 0;
            let output_service = service.clone();
            let mut output_task = tokio::spawn(async move {
                let mut lines = Vec::new();
                while let Some(bytes) = output_receive.recv().await {
                    let count = bytes.len().min(MAX_OUTPUT - size);
                    if count > 0 { let _ = output.write_all(&bytes[..count]); size += count; }
                    if lines.len() + bytes.len() <= 65536 { lines.extend_from_slice(&bytes); }
                    else { lines.clear(); }
                    while let Some(newline) = lines.iter().position(|byte| *byte == b'\n') {
                        let line: Vec<_> = lines.drain(..=newline).collect();
                        let events = crate::harnesses::normalize(&line);
                        if !events.is_empty() {
                            let _ = output_service.runtime.lock().await.registry.update(id, |session| {
                                for mut event in events {
                                    if session.execution.harness_events.len() >= 256 { session.execution.harness_events.remove(0); }
                                    Registry::record_event(session, &event.kind);
                                    event.sequence = session.events.last().map_or(0, |event|event.sequence);
                                    session.execution.harness_events.push(event);
                                }
                                Ok(())
                            });
                        }
                    }
                }
                let _ = output.sync_all();
            });
            let mut wait = wait;
            let (exit_code, reason) = tokio::select! {
                result = &mut wait => (result.ok().and_then(Result::ok), "process_exited"),
                _ = cancelled.changed() => (None, "process_cancelled"),
                () = tokio::time::sleep(Duration::from_secs(session.spec.runtime.budget.timeout_seconds)) => (None, "process_timeout"),
            };
            process::signal_group(&identity, Signal::SIGTERM);
            if exit_code.is_none() { tokio::time::sleep(Duration::from_millis(200)).await; }
            let cleaned = tokio::task::spawn_blocking(move || process::recover_agent(&recovery)).await.is_ok_and(|result| result.is_ok());
            if !wait.is_finished() { let _ = tokio::time::timeout(Duration::from_secs(2), &mut wait).await; }
            let cleaned = cleaned && wait.is_finished();
            if tokio::time::timeout(Duration::from_secs(2), &mut output_task).await.is_err() {
                output_task.abort(); let _ = output_task.await;
            }
            for task in io_tasks { task.abort(); }
            let reason = if cleaned {reason} else {"process_cleanup_failed"};
            service.revoke_inference(id).await;
            let mut runtime = service.runtime.lock().await;
            runtime.processes.remove(&id);
            let _ = runtime.registry.update(id, |s| {
                s.execution.exit_code = exit_code;
                s.execution.cleanup_pending = !cleaned;
                s.execution.phase = WorkloadPhase::Cleanup;
                if reason != "process_exited" || exit_code != Some(0) { s.execution.failure_reason = Some(reason.into()); }
                if let Some(op) = s.execution.operations.iter_mut().find(|op| op.id == operation_id) {
                    op.status = if exit_code == Some(0) { OperationStatus::Completed } else { OperationStatus::Failed };
                }
                Registry::record_event(s, reason);
                Ok(())
            });
            if let Ok(s) = runtime.registry.get(id) {
                if !s.state.is_terminal() {
                    if exit_code == Some(0) && cleaned {
                        let _ = runtime.registry.transition(id, SessionState::Completing, "process_completing");
                        let _ = runtime.registry.transition(id, SessionState::Completed, "process_completed");
                    } else { let _ = runtime.registry.transition(id, SessionState::Failed, reason); }
                }
            }
            // Children belong to the parent's lifetime even when its process
            // exits normally or reaches its deadline without an explicit stop.
            let _ = crate::orchestration::cancel_tree(&mut runtime, id, false, "parent_exited");
            drop(runtime);
            let _=service.finish_gpu(id, exit_code==Some(0) && cleaned).await;
            let _=service.release_guard_sessions().await;
        });
        guard.0 = None;
        Ok(json!({"session_id":id,"pid":pid,"operation_id":operation_id}))
    }

    pub(crate) async fn process_input(&self, id: Uuid, input: String) -> Result<Value> {
        ensure!(
            input.len() <= cerynth_agent_api::MAX_PROMPT_BYTES,
            "input exceeds limit"
        );
        let sender = self
            .runtime
            .lock()
            .await
            .processes
            .get(&id)
            .context("session has no running process")?
            .input
            .clone();
        sender
            .try_send(input.into_bytes())
            .context("input backpressure; retry later")?;
        Ok(json!({"accepted":true}))
    }

    pub(crate) async fn output(&self, id: Uuid, offset: u64, limit: usize) -> Result<Value> {
        self.runtime.lock().await.registry.get(id)?;
        ensure!(
            limit > 0 && limit <= 32 * 1024,
            "output limit must be 1..32768"
        );
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(self.directory.join("outputs").join(id.to_string()))?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.take(limit as u64).read_to_end(&mut bytes)?;
        Ok(
            json!({"offset":offset,"next_offset":offset + bytes.len() as u64,
            "text":String::from_utf8_lossy(&bytes),"truncated_at_bytes":MAX_OUTPUT}),
        )
    }
}
