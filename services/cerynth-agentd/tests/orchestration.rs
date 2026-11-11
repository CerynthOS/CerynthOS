use anyhow::Result;
use cerynth_agent_api::{
    ArtifactKind, HarnessKind, ModelConfig, ModelMode, Operation, RuntimeSpec, Session, SessionSpec,
};
use cerynth_agentd::{call, serve};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

fn spec(path: &Path) -> SessionSpec {
    SessionSpec {
        name: "orchestration".into(),
        harness: HarnessKind::GenericPty,
        workspace: path.into(),
        model: ModelConfig {
            name: "none".into(),
            mode: ModelMode::Local,
            endpoint_id: None,
            cloud_consent: false,
            disable_thinking: false,
        },
        runtime: RuntimeSpec {
            capabilities: std::collections::BTreeSet::from(["message.share".into()]),
            ..RuntimeSpec::default()
        },
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn sharing_requires_owner_authorization_and_artifacts_are_snapshots_beneath_workspace()
-> Result<()> {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let workspace = dir.path().join("workspace");
    std::fs::create_dir(&workspace)?;
    let directory = dir.path().join("state");
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let path = directory.clone();
    let daemon = tokio::spawn(async move {
        serve(path, "http://127.0.0.1:1", async {
            let _ = stopped.await;
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !directory.join("agentd.sock").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    let first: Session = serde_json::from_value(
        call(
            &directory,
            Operation::Create {
                spec: spec(&workspace),
            },
        )
        .await?,
    )?;
    let second: Session = serde_json::from_value(
        call(
            &directory,
            Operation::Create {
                spec: spec(&workspace),
            },
        )
        .await?,
    )?;
    assert!(
        call(
            &directory,
            Operation::SendMessage {
                sender: first.id,
                recipient: second.id,
                content: "unapproved".into()
            }
        )
        .await
        .is_err()
    );
    call(
        &directory,
        Operation::AuthorizeSharing {
            left: first.id,
            right: second.id,
        },
    )
    .await?;
    call(
        &directory,
        Operation::SendMessage {
            sender: first.id,
            recipient: second.id,
            content: "lower-trust task context".into(),
        },
    )
    .await?;
    let messages = call(&directory, Operation::Messages { id: second.id }).await?;
    assert_eq!(messages[0]["sender"], first.id.to_string());
    let after: Session =
        serde_json::from_value(call(&directory, Operation::Status { id: second.id }).await?)?;
    assert_eq!(
        after.spec.runtime.capabilities,
        first.spec.runtime.capabilities
    );
    std::fs::write(workspace.join("result.txt"), "original")?;
    let artifact = call(
        &directory,
        Operation::CollectArtifact {
            id: first.id,
            path: PathBuf::from("result.txt"),
            kind: ArtifactKind::TaskResult,
        },
    )
    .await?;
    let snapshot = directory
        .join("artifacts")
        .join(artifact["id"].as_str().unwrap());
    std::fs::write(workspace.join("result.txt"), "changed")?;
    assert_eq!(std::fs::read_to_string(snapshot)?, "original");
    std::fs::write(dir.path().join("outside.txt"), "private")?;
    std::os::unix::fs::symlink(dir.path().join("outside.txt"), workspace.join("link"))?;
    for path in ["../outside.txt", "link", "/etc/passwd"] {
        assert!(
            call(
                &directory,
                Operation::CollectArtifact {
                    id: first.id,
                    path: PathBuf::from(path),
                    kind: ArtifactKind::File
                }
            )
            .await
            .is_err()
        );
    }
    stop.send(()).ok();
    daemon.await??;
    Ok(())
}

#[tokio::test]
async fn observed_process_survives_control_stop_and_registry_recovery() -> Result<()> {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let path = dir.path().to_path_buf();
    let daemon = tokio::spawn(async move {
        serve(path, "http://127.0.0.1:1", async {
            let _ = stopped.await;
        })
        .await
    });
    while !dir.path().join("agentd.sock").exists() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let mut child = tokio::process::Command::new("/usr/bin/sleep")
        .arg("60")
        .kill_on_drop(true)
        .spawn()?;
    let session: Session = serde_json::from_value(
        call(
            dir.path(),
            Operation::Create {
                spec: spec(dir.path()),
            },
        )
        .await?,
    )?;
    let observed = call(
        dir.path(),
        Operation::Observe {
            id: session.id,
            pid: child.id().unwrap(),
        },
    )
    .await?;
    assert_eq!(observed["execution"]["mode"], "observed");
    stop.send(()).ok();
    daemon.await??;
    let recovered = cerynth_agentd::AgentService::open(dir.path(), "http://127.0.0.1:1")?;
    assert!(
        child.try_wait()?.is_none(),
        "observing a process must never grant termination authority"
    );
    drop(recovered);
    child.kill().await?;
    child.wait().await?;
    Ok(())
}
