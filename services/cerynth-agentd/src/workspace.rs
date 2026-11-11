//! Git metadata mutations are trusted management operations. Checkout code and
//! repository filters never run with the manager's host filesystem authority.
use crate::{process, registry::Registry};
use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{
    ExecutionBoundary, HarnessKind, IsolationPolicy, LaunchRequest, NetworkPolicy, Session,
    SessionMode, SessionState, WorkloadPhase,
};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;
use uuid::Uuid;

fn metadata_git(workspace: &Path, args: &[&str]) -> Result<Command> {
    let mut payload = vec![
        "/usr/bin/git".into(),
        "--no-pager".into(),
        "-C".into(),
        workspace.to_string_lossy().into_owned(),
    ];
    payload.extend(
        [
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "gc.auto=0",
            "-c",
            "maintenance.auto=false",
        ]
        .map(str::to_owned),
    );
    payload.extend(args.iter().map(|s| (*s).to_owned()));
    let protected = process::parent_payload(payload, std::process::id());
    ensure!(
        Path::new(&protected[0]).is_file(),
        "worktree management requires setpriv --pdeathsig"
    );
    let mut command = Command::new(&protected[0]);
    command
        .args(&protected[1..])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    Ok(command)
}

async fn register(
    session: &Session,
    path: &Path,
    branch: &str,
    control_directory: &Path,
) -> Result<(PathBuf, PathBuf)> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        metadata_git(
            &session.spec.workspace,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?
        .output(),
    )
    .await??;
    ensure!(
        output.status.success() && output.stdout.len() < 8192,
        "cannot resolve repository metadata"
    );
    let common = PathBuf::from(std::str::from_utf8(&output.stdout)?.trim()).canonicalize()?;
    ensure!(
        !control_directory.starts_with(&common) && !common.starts_with(control_directory),
        "repository metadata overlaps the control directory"
    );
    let status = tokio::time::timeout(
        Duration::from_secs(10),
        metadata_git(
            &session.spec.workspace,
            &[
                "worktree",
                "add",
                "--no-checkout",
                "-b",
                branch,
                path.to_str().context("non-UTF8 workspace")?,
                "HEAD",
            ],
        )?
        .stdout(Stdio::null())
        .status(),
    )
    .await??;
    ensure!(
        status.success(),
        "git worktree registration failed; inspect retained branches before retrying"
    );
    let marker = std::fs::read_to_string(path.join(".git"))?;
    let metadata = PathBuf::from(
        marker
            .strip_prefix("gitdir: ")
            .context("invalid worktree metadata")?
            .trim(),
    )
    .canonicalize()?;
    ensure!(
        metadata.starts_with(common.join("worktrees")) && metadata != common.join("worktrees"),
        "worktree metadata is outside the repository"
    );
    Ok((common, metadata))
}

fn checkout_plan(
    session: &Session,
    path: &Path,
    common: &Path,
    metadata: &Path,
) -> Result<(Uuid, process::PreparedLaunch)> {
    let binary = std::env::var_os("CERYNTH_WORKTREE_HELPER")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_exe()?.with_file_name("cerynth-worktree-checkout"))
        .canonicalize()
        .context("build/install cerynth-worktree-checkout beside cerynth-agentd")?;
    let mut helper = session.clone();
    helper.id = Uuid::new_v4();
    helper.spec.workspace = path.into();
    helper.spec.harness = HarnessKind::GenericPty;
    helper.execution.worktree = None;
    helper.spec.runtime.isolation = IsolationPolicy::Required;
    helper.spec.runtime.require_cgroup = true;
    helper.spec.runtime.network = NetworkPolicy::Deny;
    let launch = LaunchRequest {
        program: binary,
        args: Vec::new(),
        initial_input: None,
        pty: false,
    };
    let mut plan = process::prepare(&helper, &launch, false)?;
    let separator = plan
        .args
        .iter()
        .rposition(|s| s == "--")
        .context("sandbox delimiter missing")?;
    let mounts = vec![
        "--ro-bind".into(),
        common.to_string_lossy().into_owned(),
        common.to_string_lossy().into_owned(),
        "--bind".into(),
        metadata.to_string_lossy().into_owned(),
        metadata.to_string_lossy().into_owned(),
    ];
    plan.args.splice(separator..separator, mounts);
    plan.environment.extend([
        ("CERYNTH_WORKTREE_CHECKOUT".into(), "1".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_ALLOW_PROTOCOL".into(), String::new()),
        ("GIT_NO_LAZY_FETCH".into(), "1".into()),
    ]);
    process::protect_parent(&mut plan)?;
    Ok((helper.id, plan))
}

pub(crate) async fn create(
    registry: &mut Registry,
    session: &Session,
    path: &Path,
    branch: &str,
    control_directory: &Path,
) -> Result<()> {
    let (common, metadata) = register(session, path, branch, control_directory).await?;
    let (allocation, plan) = checkout_plan(session, path, &common, &metadata)?;
    registry.update(session.id, |s| {
        s.execution.allocation_id = Some(allocation);
        s.execution.cgroup_unit.clone_from(&plan.unit);
        s.execution.cleanup_pending = true;
        Ok(())
    })?;
    let mut child = Command::new(&plan.program)
        .args(&plan.args)
        .env_clear()
        .envs(plan.environment)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let identity = process::identity(child.id().context("checkout PID unavailable")?)?;
    registry.update(session.id, |s| {
        s.execution.process = Some(identity);
        s.execution.mode = SessionMode::Managed;
        s.boundary = ExecutionBoundary::BubblewrapSystemd;
        Ok(())
    })?;
    let result = tokio::time::timeout(Duration::from_secs(30), child.wait()).await;
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    let owned = registry.get(session.id)?;
    tokio::task::spawn_blocking(move || process::recover(&owned)).await??;
    registry.update(session.id, |s| {
        s.execution.cleanup_pending = false;
        s.execution.process = None;
        s.execution.cgroup_unit = None;
        s.execution.mode = SessionMode::Integrated;
        s.boundary = ExecutionBoundary::UnenforcedPrototype;
        s.execution.phase = WorkloadPhase::Idle;
        Ok(())
    })?;
    ensure!(
        result.context("checkout deadline exceeded")??.success(),
        "isolated checkout failed; branches and files retained for review"
    );
    Ok(())
}

pub(crate) fn begin(registry: &mut Registry, session: &Session, path: &Path) -> Result<()> {
    registry.transition(session.id, SessionState::Preparing, "workspace_preparing")?;
    registry.update(session.id, |s| {
        s.execution.worktree = Some(path.into());
        s.execution.workspace_preparing = true;
        s.execution.phase = WorkloadPhase::Tool;
        Ok(())
    })?;
    Ok(())
}
