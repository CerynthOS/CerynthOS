//! Explicit live qualification of the owner APIs, inside disposable boundaries.
use anyhow::{Context, Result, ensure};
use cerynth_platform_resources::{CgroupRoot, SessionCgroup};
use cerynthd::resources::enforcement::apply_limits;
use cerynthd::resources::{AccountingService, AgentRegistration, RegistrationStore};
use cerynthd::storage::worktree::DiscoveredWorktree;
use cerynthd::storage::{MountManager, VolumeManager};
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use uuid::Uuid;

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn worker(group: &SessionCgroup, workload: &str) -> Result<Worker> {
    let code = format!(
        "open({},'w').write('0');print('ready',flush=True);{}",
        serde_json::to_string(&group.path().join("cgroup.procs"))?,
        workload
    );
    let mut child = Command::new("/usr/bin/python3")
        .args(["-I", "-c", &code])
        .stdout(Stdio::piped())
        .spawn()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take().context("worker pipe")?).read_line(&mut line)?;
    let owned = Worker(child);
    ensure!(
        line.trim() == "ready",
        "worker failed before joining cgroup"
    );
    Ok(owned)
}
fn registration(group: &SessionCgroup, quota: u64) -> AgentRegistration {
    let mut registration = AgentRegistration::new(group.session_id(), group.path());
    registration.cpu_quota_us = Some(quota);
    registration.memory_limit_bytes = Some(64 * 1024 * 1024);
    registration
}
fn resources() -> Result<serde_json::Value> {
    ensure!(
        fs::read_to_string("/sys/kernel/sched_ext/state")
            .unwrap_or_default()
            .trim()
            != "enabled",
        "live quota qualification requires Linux default scheduling"
    );
    let line = fs::read_to_string("/proc/self/cgroup")?;
    let relative = line
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .context("unified cgroup")?;
    ensure!(
        relative
            .split('/')
            .any(|part| part.starts_with("cerynth-native-check-") && part.ends_with(".service")),
        "run in an explicitly delegated qualification service"
    );
    let delegated = PathBuf::from("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
    let supervisor = delegated.join("supervisor");
    fs::create_dir(&supervisor)?;
    fs::write(supervisor.join("cgroup.procs"), "0")?;
    let root = CgroupRoot::at(&delegated)?;
    let enabled = root.enable_controllers(&["cpu", "memory", "pids"]);
    ensure!(
        enabled.failed.is_empty(),
        "required controllers not delegated: {:?}",
        enabled.failed
    );
    let cpu = SessionCgroup::create(&root, Uuid::new_v4())?;
    let record = registration(&cpu, 10_000);
    ensure!(
        apply_limits(&record)?.fully_enforced(),
        "kernel did not apply requested CPU/RAM limits"
    );
    let mut store = RegistrationStore::new();
    store.register(record)?;
    let accounting = AccountingService::new(store);
    let mut load = worker(
        &cpu,
        "exec('data=bytearray(12*1024**2)\\nwhile True: sum(range(10000))')",
    )?;
    std::thread::sleep(Duration::from_secs(3));
    let usage = accounting.agent_usage(cpu.session_id())?;
    ensure!(
        usage.kernel_backed
            && usage.populated
            && usage.cpu_throttled_periods > 0
            && usage.cpu_usage_usec > 100_000
            && usage.cpu_usage_usec < 900_000,
        "CPU cap/progress not observed: {usage:?}"
    );
    ensure!(
        usage.memory_current_bytes >= 12 * 1024 * 1024 && !usage.pids.is_empty(),
        "memory/PID attribution missing"
    );
    load.0.kill()?;
    load.0.wait()?;
    drop(load);
    cpu.remove()?;
    let memory = SessionCgroup::create(&root, Uuid::new_v4())?;
    let record = registration(&memory, 100_000);
    ensure!(
        apply_limits(&record)?.fully_enforced(),
        "memory limit not applied"
    );
    // Isolate resident-memory containment from host swap capacity. This is
    // confined to the explicitly delegated disposable fixture cgroup.
    fs::write(memory.path().join("memory.swap.max"), "0")?;
    let mut load = worker(&memory, "data=bytearray(256*1024**2)")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = load.0.try_wait()? {
            break status;
        }
        ensure!(Instant::now() < deadline, "contained OOM deadline exceeded");
        std::thread::sleep(Duration::from_millis(20));
    };
    let memory_usage = memory.usage()?;
    ensure!(
        status.signal() == Some(9) && memory_usage.memory.events.oom_kill > 0,
        "no kernel-backed contained OOM"
    );
    let result = json!({"kind":"live-native-resource-owner-api", "cpu_usage_usec": usage.cpu_usage_usec,
        "cpu_throttled_periods":usage.cpu_throttled_periods,"observed_memory_bytes":usage.memory_current_bytes,
        "observed_pids":usage.pids,"contained_oom_kills":memory_usage.memory.events.oom_kill,
        "kernel_limits_applied":true,"owner_process_survived":true,"systemd_delegated_fixture_only":true});
    drop(load);
    memory.remove()?;
    Ok(result)
}
#[allow(clippy::too_many_lines)] // Keep setup, crash injection, verification and cleanup in one fixture.
fn storage() -> Result<serde_json::Value> {
    let root = std::env::temp_dir().join(format!("cerynth-live-storage-α {}", Uuid::new_v4()));
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let authority = root.join("authority");
    let workspace = root.join("worktree");
    fs::create_dir(&workspace)?;
    let source = DiscoveredWorktree {
        supplied_path: workspace.clone(),
        resolved_path: workspace,
        permitted_root: root.clone(),
        observed_at: 0,
        is_git_worktree: false,
    };
    let id = Uuid::new_v4();
    let size = 32 * 1024 * 1024;
    let mut manager = Some(VolumeManager::init(&authority, 128 * 1024 * 1024)?);
    let record =
        manager
            .as_mut()
            .unwrap()
            .create_volume(id, &source, size, 64 * 1024 * 1024, true)?;
    ensure!(
        Command::new("/usr/bin/mkfs.ext4")
            .args(["-q", "-F"])
            .arg(&record.image_path)
            .status()?
            .success(),
        "format disposable filesystem"
    );
    manager.as_mut().unwrap().mark_ready(id)?;
    let mounts = MountManager::new(&authority);
    drop(manager.take());
    let mut owner = Worker(
        Command::new(std::env::current_exe()?)
            .arg("storage-owner")
            .arg(&authority)
            .arg(id.to_string())
            .stdout(Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while !record.mount_path.join("owner-ready").exists() {
        ensure!(
            owner.0.try_wait()?.is_none(),
            "storage owner exited before mount readiness"
        );
        ensure!(
            Instant::now() < deadline,
            "storage owner readiness deadline"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    owner.0.kill()?;
    owner.0.wait()?;
    drop(owner);
    manager = Some(VolumeManager::load(&authority)?);
    let result = (|| -> Result<serde_json::Value> {
        let sentinel = record.mount_path.join("sentinel");
        ensure!(
            manager.as_mut().unwrap().recover_stale_mounts()?.is_empty(),
            "healthy mount incorrectly recovered"
        );
        ensure!(
            fs::read(&sentinel)? == b"persistent live FUSE data\n",
            "restart persistence"
        );
        ensure!(
            Command::new("/usr/bin/systemctl")
                .args([
                    "--user",
                    "kill",
                    "--signal=SIGKILL",
                    &mounts.helper_unit(&id)
                ])
                .status()?
                .success(),
            "kill exact owned helper"
        );
        std::thread::sleep(Duration::from_millis(100));
        ensure!(
            mounts.recover_stale_mounts()? == vec![id],
            "stale kernel mount not cleaned"
        );
        ensure!(
            manager.as_mut().unwrap().recover_stale_mounts()? == vec![id],
            "stale metadata not reconciled"
        );
        mounts.mount_volume(&id, size)?;
        manager.as_mut().unwrap().mark_mounted(id)?;
        ensure!(
            fs::read(&sentinel)? == b"persistent live FUSE data\n",
            "helper crash lost data"
        );
        mounts.unmount_volume(&id)?;
        manager.as_mut().unwrap().mark_unmounted(id)?;
        manager.as_mut().unwrap().remove_volume(id)?;
        ensure!(
            manager.as_ref().unwrap().list_volumes()?.is_empty() && !record.image_path.exists(),
            "volume cleanup incomplete"
        );
        Ok(
            json!({"kind":"live-native-volume-owner-api","actual_fuse_mount":true,"healthy_mount_preserved_across_owner_sigkill":true,
            "helper_crash_recovery":true,"sentinel_persisted":true,"unicode_and_space_mount_path":true,"cleanup_complete":true}),
        )
    })();
    let _ = mounts.unmount_volume(&id);
    drop(manager.take());
    if result.is_ok() {
        fs::remove_dir_all(&root)?;
    }
    result
}
fn storage_owner() -> Result<serde_json::Value> {
    let root = PathBuf::from(std::env::args().nth(2).context("owner authority")?);
    let id = Uuid::parse_str(&std::env::args().nth(3).context("owner volume")?)?;
    let mut owner = VolumeManager::load(&root)?;
    let record = owner.get_volume(id)?;
    MountManager::new(&root).mount_volume(&id, record.image_bytes)?;
    owner.mark_mounted(id)?;
    let mut file = fs::File::create(record.mount_path.join("sentinel"))?;
    file.write_all(b"persistent live FUSE data\n")?;
    file.sync_all()?;
    fs::write(record.mount_path.join("owner-ready"), b"ready")?;
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn main() -> Result<()> {
    ensure!(
        std::env::var("CERYNTH_RUN_LIVE_PLATFORM").as_deref() == Ok("1"),
        "explicit live qualification opt-in required"
    );
    let mode = std::env::args()
        .nth(1)
        .context("select resources or storage")?;
    let report = match mode.as_str() {
        "resources" => resources()?,
        "storage" => storage()?,
        "storage-owner" => storage_owner()?,
        _ => anyhow::bail!("unknown qualification mode"),
    };
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
