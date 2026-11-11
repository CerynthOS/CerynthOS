use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{IsolationPolicy, LaunchRequest, NetworkPolicy, ProcessIdentity, Session};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Serialize)]
pub struct ProcessSample {
    pub pid: u32,
    pub parent_pid: u32,
    pub process_group: i32,
    pub start_ticks: u64,
    pub rss_bytes: Option<u64>,
    pub command: String,
}

pub fn sample(pid: u32) -> Result<ProcessSample> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let end = stat.rfind(')').context("invalid proc stat")?;
    let command_begin = stat.find('(').context("invalid proc stat")?;
    let fields: Vec<_> = stat[end + 1..].split_whitespace().collect();
    ensure!(fields.len() > 19, "incomplete proc stat");
    let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
    let rss = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|line| line.split_whitespace().next())
        .and_then(|v| v.parse::<u64>().ok())
        .and_then(|kb| kb.checked_mul(1024));
    Ok(ProcessSample {
        pid,
        parent_pid: fields[1].parse()?,
        process_group: fields[2].parse()?,
        start_ticks: fields[19].parse()?,
        rss_bytes: rss,
        command: stat[command_begin + 1..end].into(),
    })
}

pub fn identity(pid: u32) -> Result<ProcessIdentity> {
    let value = sample(pid)?;
    Ok(ProcessIdentity {
        pid,
        start_ticks: value.start_ticks,
        process_group: value.process_group,
        boot_id: fs::read_to_string("/proc/sys/kernel/random/boot_id")?
            .trim()
            .into(),
    })
}

pub fn matching(identity: &ProcessIdentity) -> bool {
    fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .is_ok_and(|boot| boot.trim() == identity.boot_id)
        && sample(identity.pid).is_ok_and(|p| {
            p.start_ticks == identity.start_ticks && p.process_group == identity.process_group
        })
}

pub fn group_members(identity: &ProcessIdentity) -> Vec<ProcessSample> {
    if fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_or(true, |boot| boot.trim() != identity.boot_id)
    {
        return Vec::new();
    }
    if sample(identity.pid).is_ok_and(|process| process.start_ticks != identity.start_ticks) {
        return Vec::new();
    }
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| sample(pid).ok())
        .filter(|p| {
            p.process_group == identity.process_group && p.start_ticks >= identity.start_ticks
        })
        .collect()
}

pub fn descendants(identity: &ProcessIdentity) -> Vec<ProcessSample> {
    if fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_or(true, |boot| boot.trim() != identity.boot_id)
        || sample(identity.pid).map_or(true, |root| root.start_ticks != identity.start_ticks)
    {
        return Vec::new();
    }
    let mut remaining: Vec<_> = fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| sample(pid).ok())
        .collect();
    let mut ids = vec![identity.pid];
    loop {
        let children: Vec<_> = remaining
            .iter()
            .filter(|p| !ids.contains(&p.pid) && ids.contains(&p.parent_pid))
            .map(|p| p.pid)
            .collect();
        if children.is_empty() {
            break;
        }
        ids.extend(children);
    }
    remaining.retain(|p| ids.contains(&p.pid));
    remaining
}

pub fn signal_group(identity: &ProcessIdentity, signal: Signal) {
    // A live group retains its group ID even after the leader exits. Do not signal
    // a group with no matching original-generation member or our own daemon group.
    if identity.process_group > 1
        && identity.process_group != nix::unistd::getpgrp().as_raw()
        && !group_members(identity).is_empty()
    {
        let _ = killpg(Pid::from_raw(identity.process_group), signal);
    }
}

pub fn unit_name(id: uuid::Uuid) -> String {
    format!("cerynth-agent-{id}.scope")
}

fn systemctl_bounded(args: &[&str]) -> Result<std::process::Output> {
    let uid = nix::unistd::getuid();
    let mut child = std::process::Command::new("/usr/bin/systemctl")
        .args(args)
        .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path=/run/user/{uid}/bus"),
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    for _ in 0..200 {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    anyhow::bail!("scope cleanup deadline exceeded; inspect the user systemd manager");
}

pub fn recover(session: &Session) -> Result<()> {
    recover_agent(session)?;
    if let Some(unit) = &session.execution.native_guard_unit {
        ensure!(
            *unit == guard_unit_name(session.id),
            "invalid owned Guard unit"
        );
        stop_scope(unit)?;
    }
    Ok(())
}

pub(crate) fn guard_unit_name(id: uuid::Uuid) -> String {
    format!("cerynth-guard-{id}.scope")
}

pub(crate) fn stop_scope(unit: &str) -> Result<()> {
    let result = systemctl_bounded(&["--user", "stop", unit])?;
    if !result.status.success() {
        let state =
            systemctl_bounded(&["--user", "show", "--property=LoadState", "--value", unit])?;
        ensure!(
            state.status.success() && state.stdout == b"not-found\n",
            "owned scope cleanup failed"
        );
    }
    Ok(())
}

pub fn recover_agent(session: &Session) -> Result<()> {
    if session.execution.mode == cerynth_agent_api::SessionMode::Observed {
        return Ok(());
    }
    // Blocking, bounded startup cleanup before accepting any new client.
    if let Some(process) = &session.execution.process {
        if matching(process) || !group_members(process).is_empty() {
            signal_group(process, Signal::SIGKILL);
        }
    }
    if let Some(unit) = &session.execution.cgroup_unit {
        ensure!(
            *unit == unit_name(session.id)
                || session
                    .execution
                    .allocation_id
                    .is_some_and(|id| *unit == unit_name(id)),
            "invalid owned resource unit"
        );
        let result = systemctl_bounded(&["--user", "stop", unit])?;
        if !result.status.success() {
            // A collected/nonexistent scope is already clean. A failed manager
            // connection is not evidence of cleanup and must fail closed.
            let state =
                systemctl_bounded(&["--user", "show", "--property=LoadState", "--value", unit])?;
            ensure!(
                state.status.success() && state.stdout == b"not-found\n",
                "scope cleanup failed; inspect the user systemd manager"
            );
        }
    }
    Ok(())
}

pub struct PreparedLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub unit: Option<String>,
}

pub(crate) fn parent_payload(payload: Vec<String>, parent: u32) -> Vec<String> {
    let mut protected = vec![
        "/usr/bin/setpriv".into(),
        "--pdeathsig".into(),
        "KILL".into(),
        "--no-new-privs".into(),
        "/usr/bin/sh".into(),
        "-c".into(),
        "[ \"$PPID\" = \"$1\" ] || exit 125; shift; exec \"$@\"".into(),
        "cerynth-parent-guard".into(),
        parent.to_string(),
    ];
    protected.extend(payload);
    protected
}

/// Arm a kernel parent-death signal before checking the original supervisor.
/// The fixed shell program passes all user argv positionally; it never evaluates
/// those arguments as shell source. A late child cannot run after daemon death.
pub fn protect_parent(plan: &mut PreparedLaunch) -> Result<()> {
    if plan.unit.is_some() {
        ensure!(
            Path::new("/usr/bin/setpriv").is_file(),
            "managed launch requires util-linux setpriv --pdeathsig"
        );
        let separator = plan
            .args
            .iter()
            .position(|arg| arg == "--")
            .context("resource wrapper is missing")?;
        let payload = plan.args.split_off(separator + 1);
        plan.args
            .extend(parent_payload(payload, std::process::id()));
    }
    Ok(())
}

const BWRAP_ARGS: &[&str] = &[
    "--die-with-parent",
    "--new-session",
    "--unshare-user",
    "--unshare-pid",
    "--unshare-ipc",
    "--unshare-uts",
    "--cap-drop",
    "ALL",
    "--ro-bind",
    "/usr",
    "/usr",
    "--symlink",
    "usr/bin",
    "/bin",
    "--symlink",
    "usr/lib",
    "/lib",
    "--symlink",
    "usr/lib",
    "/lib64",
    "--proc",
    "/proc",
    "--dev",
    "/dev",
    "--tmpfs",
    "/tmp",
    "--dir",
    "/run",
    "--dir",
    "/etc",
    "--dir",
    "/home/agent",
    "--setenv",
    "HOME",
    "/home/agent",
];

pub fn prepare(
    session: &Session,
    launch: &LaunchRequest,
    allow_unisolated: bool,
) -> Result<PreparedLaunch> {
    ensure!(
        launch.program.is_absolute()
            && launch.args.len() <= 128
            && launch
                .args
                .iter()
                .all(|arg| arg.len() <= 8192 && !arg.contains('\0')),
        "invalid command; use an absolute executable and bounded argv"
    );
    let program = launch
        .program
        .canonicalize()
        .context("executable not found")?;
    ensure!(program.is_file(), "executable must be a regular file");
    let mut args = Vec::new();
    let isolated = session.spec.runtime.isolation == IsolationPolicy::Required;
    ensure!(
        !isolated || session.spec.runtime.require_cgroup,
        "sandbox supervision requires a systemd scope for descendant cleanup"
    );
    let mut command = program.clone();
    if isolated {
        command = PathBuf::from("/usr/bin/bwrap");
        ensure!(command.is_file(), "sandbox required: install bubblewrap");
        let workspace = session
            .execution
            .worktree
            .as_ref()
            .unwrap_or(&session.spec.workspace);
        args.extend(BWRAP_ARGS.iter().map(|value| (*value).to_owned()));
        if session.spec.runtime.network == NetworkPolicy::Deny {
            args.push("--unshare-net".into());
        } else {
            // Host network is explicitly requested; this is not domain filtering.
            for source in ["/etc/resolv.conf", "/etc/hosts", "/etc/ssl/certs"] {
                if Path::new(source).exists() {
                    args.extend(["--ro-bind".into(), source.into(), source.into()]);
                }
            }
        }
        if !program.starts_with("/usr") {
            append_hermes_dependencies(session, &program, &mut args)?;
            args.extend([
                "--ro-bind".into(),
                program.to_string_lossy().into_owned(),
                program.to_string_lossy().into_owned(),
            ]);
        }
        append_protocol_dependencies(session, &program, &mut args)?;
        args.extend([
            "--bind".into(),
            workspace.to_string_lossy().into_owned(),
            workspace.to_string_lossy().into_owned(),
            "--chdir".into(),
            workspace.to_string_lossy().into_owned(),
            "--".into(),
            program.to_string_lossy().into_owned(),
        ]);
    } else {
        ensure!(
            allow_unisolated,
            "unisolated execution is disabled by daemon policy"
        );
    }
    args.extend(launch.args.clone());
    let unit = if session.spec.runtime.require_cgroup {
        let budget = &session.spec.runtime.budget;
        let mut wrapper = vec![
            "--user".into(),
            "--scope".into(),
            "--quiet".into(),
            "--collect".into(),
            format!("--unit={}", unit_name(session.id)),
            format!("--property=CPUWeight={}", budget.cpu_weight),
            format!("--property=MemoryHigh={}", budget.memory_high_bytes),
            format!("--property=MemoryMax={}", budget.memory_max_bytes),
            format!("--property=TasksMax={}", budget.tasks_max),
            "--property=KillMode=control-group".into(),
            "--".into(),
            command.to_string_lossy().into_owned(),
        ];
        wrapper.extend(args);
        command = PathBuf::from("/usr/bin/systemd-run");
        ensure!(
            command.is_file(),
            "cgroup control required: systemd-run unavailable"
        );
        args = wrapper;
        Some(unit_name(session.id))
    } else {
        None
    };
    let environment = launch_environment(session);
    Ok(PreparedLaunch {
        program: command,
        args,
        environment,
        unit,
    })
}

fn launch_environment(session: &Session) -> Vec<(String, String)> {
    let mut environment = vec![
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("LANG".into(), "C.UTF-8".into()),
        ("TERM".into(), "xterm-256color".into()),
        ("HOME".into(), "/home/agent".into()),
    ];
    if session.spec.harness == cerynth_agent_api::HarnessKind::OpenCode {
        let model = &session.spec.model.name;
        let config = serde_json::json!({"model":format!("ollama/{model}"),"small_model":format!("ollama/{model}"),"enabled_providers":["ollama"],"provider":{"ollama":{
            "npm":"@ai-sdk/openai-compatible","name":"Local Ollama",
            "options":{"baseURL":"http://127.0.0.1:11434/v1"},
            "models":{model:{"name":model,"options":{"reasoningEffort":"none"},"limit":{"context":16384,"output":2048}}}}},
            "permission":{"*":"deny","read":"allow","edit":"allow","bash":"allow","glob":"allow","grep":"allow"}});
        environment.push(("OPENCODE_CONFIG_CONTENT".into(), config.to_string()));
        environment.push(("OPENCODE_DISABLE_AUTOUPDATE".into(), "true".into()));
        environment.push(("OPENCODE_DISABLE_MODELS_FETCH".into(), "true".into()));
        environment.push(("DO_NOT_TRACK".into(), "1".into()));
    }
    if session.spec.runtime.require_cgroup {
        // Required for the user systemd bus only. Bubblewrap never mounts /run/user.
        let uid = nix::unistd::getuid();
        environment.push(("XDG_RUNTIME_DIR".into(), format!("/run/user/{uid}")));
        environment.push((
            "DBUS_SESSION_BUS_ADDRESS".into(),
            format!("unix:path=/run/user/{uid}/bus"),
        ));
    }
    if session.spec.harness == cerynth_agent_api::HarnessKind::Hermes {
        environment.push(("HERMES_HOME".into(), "/home/agent/.hermes".into()));
        environment.push(("OPENAI_API_KEY".into(), "local-no-key".into()));
    }
    environment
}

fn append_protocol_dependencies(
    session: &Session,
    program: &Path,
    args: &mut Vec<String>,
) -> Result<()> {
    if session.spec.runtime.transport != cerynth_agent_api::HarnessTransport::CliJsonl {
        ensure!(
            program == crate::harnesses::protocol_bridge()?,
            "protocol launch requires the configured bridge"
        );
        let native = crate::harnesses::executable("opencode")
            .context("OpenCode installation unavailable")?;
        if !native.starts_with("/usr") {
            args.extend([
                "--ro-bind".into(),
                native.to_string_lossy().into_owned(),
                native.to_string_lossy().into_owned(),
            ]);
        }
    }
    Ok(())
}

fn append_hermes_dependencies(
    session: &Session,
    program: &Path,
    args: &mut Vec<String>,
) -> Result<()> {
    if session.spec.harness != cerynth_agent_api::HarnessKind::Hermes {
        return Ok(());
    }

    let configured =
        crate::harnesses::executable("hermes").context("Hermes installation is not configured")?;
    ensure!(
        configured == program,
        "Hermes must use the owner-configured installation"
    );
    let root = std::env::var_os("CERYNTH_HERMES_INSTALL")
        .map(PathBuf::from)
        .or_else(|| program.ancestors().nth(3).map(Path::to_path_buf))
        .context("Hermes dependency directory is unavailable")?
        .canonicalize()?;
    ensure!(
        root.join("pyproject.toml").is_file()
            && root.join(".venv/pyvenv.cfg").is_file()
            && program.starts_with(&root),
        "Hermes requires a pinned source installation with its .venv"
    );
    args.extend([
        "--ro-bind".into(),
        root.to_string_lossy().into_owned(),
        root.to_string_lossy().into_owned(),
    ]);
    Ok(())
}
