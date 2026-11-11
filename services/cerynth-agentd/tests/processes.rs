use anyhow::{Context, Result};
use cerynth_agent_api::{
    HarnessKind, IsolationPolicy, LaunchRequest, ModelConfig, Operation, Session, SessionSpec,
    SessionState,
};
use cerynth_agentd::{call, serve_with_policy};
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};
use uuid::Uuid;

async fn wait_state(path: &Path, id: Uuid) -> Result<Session> {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let session: Session =
                serde_json::from_value(call(path, Operation::Status { id }).await?)?;
            if session.state.is_terminal() {
                return Ok::<_, anyhow::Error>(session);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("process did not reach terminal state")?
}

fn spec(path: &Path) -> SessionSpec {
    let runtime = cerynth_agent_api::RuntimeSpec {
        isolation: IsolationPolicy::Unisolated,
        require_cgroup: false,
        ..cerynth_agent_api::RuntimeSpec::default()
    };
    SessionSpec {
        name: "process-test".into(),
        harness: HarnessKind::GenericPty,
        workspace: path.into(),
        model: ModelConfig {
            name: "none".into(),
            mode: cerynth_agent_api::ModelMode::Local,
            endpoint_id: None,
            cloud_consent: false,
            disable_thinking: false,
        },
        runtime,
    }
}

// One daemon lifecycle validates IO, termination, and descendant ownership together.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn supervision_idempotency_pty_input_timeout_and_descendant_cleanup() -> Result<()> {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let path = dir.path().to_path_buf();
    let daemon = tokio::spawn(async move {
        serve_with_policy(path, "http://127.0.0.1:1", true, async {
            let _ = stopped.await;
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !dir.path().join("agentd.sock").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    let session: Session = serde_json::from_value(
        call(
            dir.path(),
            Operation::Create {
                spec: spec(dir.path()),
            },
        )
        .await?,
    )?;
    let operation_id = Uuid::new_v4();
    let launch = LaunchRequest {
        program: "/usr/bin/sh".into(),
        args: vec!["-c".into(), "printf supervised; sleep 0.1".into()],
        initial_input: None,
        pty: false,
    };
    call(
        dir.path(),
        Operation::Start {
            id: session.id,
            operation_id,
            launch: launch.clone(),
        },
    )
    .await?;
    let completed = wait_state(dir.path(), session.id).await?;
    assert_eq!(completed.state, SessionState::Completed);
    assert_eq!(completed.execution.exit_code, Some(0));
    let replay = call(
        dir.path(),
        Operation::Start {
            id: session.id,
            operation_id,
            launch,
        },
    )
    .await?;
    assert_eq!(replay["action_executed"], false);
    assert!(
        call(
            dir.path(),
            Operation::Start {
                id: session.id,
                operation_id,
                launch: LaunchRequest {
                    program: "/usr/bin/false".into(),
                    args: vec![],
                    initial_input: None,
                    pty: false
                }
            }
        )
        .await
        .is_err()
    );
    let output = call(
        dir.path(),
        Operation::Output {
            id: session.id,
            offset: 0,
            limit: 1024,
        },
    )
    .await?;
    assert!(output["text"].as_str().unwrap().contains("supervised"));

    let session: Session = serde_json::from_value(
        call(
            dir.path(),
            Operation::Create {
                spec: spec(dir.path()),
            },
        )
        .await?,
    )?;
    call(
        dir.path(),
        Operation::Start {
            id: session.id,
            operation_id: Uuid::new_v4(),
            launch: LaunchRequest {
                program: "/usr/bin/sh".into(),
                args: vec!["-c".into(), "read line; printf 'pty:%s' \"$line\"".into()],
                initial_input: None,
                pty: true,
            },
        },
    )
    .await?;
    call(
        dir.path(),
        Operation::Input {
            id: session.id,
            input: "hello\n".into(),
        },
    )
    .await?;
    assert_eq!(
        wait_state(dir.path(), session.id).await?.state,
        SessionState::Completed
    );
    let output = call(
        dir.path(),
        Operation::Output {
            id: session.id,
            offset: 0,
            limit: 1024,
        },
    )
    .await?;
    assert!(output["text"].as_str().unwrap().contains("pty:hello"));

    let mut timed = spec(dir.path());
    timed.runtime.budget.timeout_seconds = 1;
    let session: Session =
        serde_json::from_value(call(dir.path(), Operation::Create { spec: timed }).await?)?;
    call(
        dir.path(),
        Operation::Start {
            id: session.id,
            operation_id: Uuid::new_v4(),
            launch: LaunchRequest {
                program: "/usr/bin/sh".into(),
                args: vec!["-c".into(), "sleep 60 & echo $! > child.pid; wait".into()],
                initial_input: None,
                pty: false,
            },
        },
    )
    .await?;
    let failed = wait_state(dir.path(), session.id).await?;
    assert_eq!(failed.state, SessionState::Failed);
    assert_eq!(
        failed.execution.failure_reason.as_deref(),
        Some("process_timeout")
    );
    let child: u32 = std::fs::read_to_string(dir.path().join("child.pid"))?
        .trim()
        .parse()?;
    let stat = std::fs::read_to_string(format!("/proc/{child}/stat"));
    assert!(
        stat.is_err() || stat.unwrap().contains(") Z "),
        "descendant is still running"
    );
    stop.send(()).ok();
    daemon.await??;
    Ok(())
}

#[tokio::test]
async fn delegation_rejects_authority_and_aggregate_budget_escalation() -> Result<()> {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let path = dir.path().to_path_buf();
    let daemon = tokio::spawn(async move {
        serve_with_policy(path, "http://127.0.0.1:1", false, async {
            let _ = stopped.await;
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !dir.path().join("agentd.sock").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    let parent: Session = serde_json::from_value(
        call(
            dir.path(),
            Operation::Create {
                spec: spec(dir.path()),
            },
        )
        .await?,
    )?;
    let mut child = spec(dir.path());
    child.runtime.parent_id = Some(parent.id);
    child.runtime.capabilities.insert("host.root".into());
    assert!(
        call(
            dir.path(),
            Operation::Create {
                spec: child.clone()
            }
        )
        .await
        .is_err()
    );
    child.runtime.capabilities.clear();
    call(
        dir.path(),
        Operation::Create {
            spec: child.clone(),
        },
    )
    .await?;
    assert!(
        call(dir.path(), Operation::Create { spec: child })
            .await
            .is_err()
    );
    stop.send(()).ok();
    daemon.await??;
    Ok(())
}

async fn ready(path: &Path) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if call(path, Operation::List).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    Ok(())
}

// Exercise a killed daemon, its orphaned child, archive persistence and durable
// operation IDs with real processes rather than a mocked lifecycle state.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn crash_recovery_kills_owned_generation_and_archive_preserves_operation_ids() -> Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    let executable = env!("CARGO_BIN_EXE_cerynth-agentd");
    let mut daemon = tokio::process::Command::new(executable)
        .args([
            "--directory",
            dir.path().to_str().unwrap(),
            "serve",
            "--allow-unisolated",
        ])
        .kill_on_drop(true)
        .spawn()?;
    ready(dir.path()).await?;
    let created: Session = serde_json::from_value(
        call(
            dir.path(),
            Operation::Create {
                spec: spec(dir.path()),
            },
        )
        .await?,
    )?;
    let operation_id = Uuid::new_v4();
    let launch = LaunchRequest {
        program: "/usr/bin/sleep".into(),
        args: vec!["60".into()],
        initial_input: None,
        pty: false,
    };
    call(
        dir.path(),
        Operation::Start {
            id: created.id,
            operation_id,
            launch: launch.clone(),
        },
    )
    .await?;
    let running: Session =
        serde_json::from_value(call(dir.path(), Operation::Status { id: created.id }).await?)?;
    let pid = running.execution.process.as_ref().unwrap().pid;
    daemon.kill().await?;
    daemon.wait().await?;
    let mut recovered = tokio::process::Command::new(executable)
        .args([
            "--directory",
            dir.path().to_str().unwrap(),
            "serve",
            "--allow-unisolated",
        ])
        .kill_on_drop(true)
        .spawn()?;
    ready(dir.path()).await?;
    let status: Session =
        serde_json::from_value(call(dir.path(), Operation::Status { id: created.id }).await?)?;
    assert_eq!(status.state, SessionState::Failed);
    assert_eq!(
        status.execution.operations[0].status,
        cerynth_agent_api::OperationStatus::Unknown
    );
    assert!(!status.execution.cleanup_pending);
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
    assert!(
        stat.is_err() || stat.unwrap().contains(") Z "),
        "recovered process is still alive"
    );
    call(dir.path(), Operation::Archive { id: created.id }).await?;
    let archived: Session =
        serde_json::from_value(call(dir.path(), Operation::Status { id: created.id }).await?)?;
    assert_eq!(archived.state, SessionState::Failed);
    assert_eq!(
        call(dir.path(), Operation::List).await?,
        serde_json::json!([])
    );
    let replay = call(
        dir.path(),
        Operation::Start {
            id: created.id,
            operation_id,
            launch: launch.clone(),
        },
    )
    .await?;
    assert_eq!(replay["action_executed"], false);
    let next: Session = serde_json::from_value(
        call(
            dir.path(),
            Operation::Create {
                spec: spec(dir.path()),
            },
        )
        .await?,
    )?;
    assert!(
        call(
            dir.path(),
            Operation::Start {
                id: next.id,
                operation_id,
                launch
            }
        )
        .await
        .is_err()
    );
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(recovered.id().unwrap())?),
        nix::sys::signal::Signal::SIGTERM,
    )?;
    tokio::time::timeout(Duration::from_secs(5), recovered.wait()).await??;
    Ok(())
}

#[tokio::test]
async fn parent_completion_cancels_child_and_shared_workspace_is_rejected() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let state = dir.path().join("state");
    let parent_workspace = dir.path().join("parent");
    let child_workspace = dir.path().join("child");
    std::fs::create_dir(&parent_workspace)?;
    std::fs::create_dir(&child_workspace)?;
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let path = state.clone();
    let daemon = tokio::spawn(async move {
        serve_with_policy(path, "http://127.0.0.1:1", true, async {
            let _ = stopped.await;
        })
        .await
    });
    ready(&state).await?;
    let parent: Session = serde_json::from_value(
        call(
            &state,
            Operation::Create {
                spec: spec(&parent_workspace),
            },
        )
        .await?,
    )?;
    let mut child_spec = spec(&child_workspace);
    child_spec.runtime.parent_id = Some(parent.id);
    let child: Session =
        serde_json::from_value(call(&state, Operation::Create { spec: child_spec }).await?)?;
    let launch = LaunchRequest {
        program: "/usr/bin/sleep".into(),
        args: vec!["60".into()],
        initial_input: None,
        pty: false,
    };
    call(
        &state,
        Operation::Start {
            id: child.id,
            operation_id: Uuid::new_v4(),
            launch: launch.clone(),
        },
    )
    .await?;
    let conflicting: Session = serde_json::from_value(
        call(
            &state,
            Operation::Create {
                spec: spec(&child_workspace),
            },
        )
        .await?,
    )?;
    assert!(
        call(
            &state,
            Operation::Start {
                id: conflicting.id,
                operation_id: Uuid::new_v4(),
                launch
            }
        )
        .await
        .is_err()
    );
    call(
        &state,
        Operation::Start {
            id: parent.id,
            operation_id: Uuid::new_v4(),
            launch: LaunchRequest {
                program: "/usr/bin/sleep".into(),
                args: vec!["0.1".into()],
                initial_input: None,
                pty: false,
            },
        },
    )
    .await?;
    assert_eq!(
        wait_state(&state, parent.id).await?.state,
        SessionState::Completed
    );
    assert_eq!(
        wait_state(&state, child.id).await?.state,
        SessionState::Cancelled
    );
    stop.send(()).ok();
    daemon.await??;
    Ok(())
}
