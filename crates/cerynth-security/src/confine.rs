//! In-process confinement applied by `cerynth-guard-exec` just before it
//! exec's the agent: Landlock (L4) then seccomp (L5). Both are inherited by
//! every child and can't be removed afterwards.

use std::collections::BTreeMap;
use std::path::PathBuf;

use landlock::{
    ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetStatus,
};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

/// What Landlock managed to enforce on this kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandlockLevel {
    Full,
    Partial,
    Unsupported,
}

/// Restrict the filesystem to `ro` (read and execute) and `rw` (everything).
/// Paths that don't exist are skipped.
///
/// # Errors
/// Fails if a ruleset can't be built. On kernels without Landlock this
/// returns `Ok(Unsupported)`; the caller decides whether that is acceptable.
pub fn apply_landlock(ro: &[PathBuf], rw: &[PathBuf]) -> anyhow::Result<LandlockLevel> {
    let abi = ABI::V5;
    let read = AccessFs::from_read(abi);
    let all = AccessFs::from_all(abi);
    let mut ruleset = Ruleset::default().handle_access(all)?.create()?;
    for (paths, access) in [(ro, read), (rw, all)] {
        for p in paths {
            if let Ok(fd) = PathFd::new(p) {
                ruleset = ruleset.add_rule(PathBeneath::new(fd, access))?;
            }
        }
    }
    let status = ruleset.restrict_self()?;
    Ok(match status.ruleset {
        RulesetStatus::FullyEnforced => LandlockLevel::Full,
        RulesetStatus::PartiallyEnforced => LandlockLevel::Partial,
        RulesetStatus::NotEnforced => LandlockLevel::Unsupported,
    })
}

/// Syscalls an agent never needs and that widen the kernel attack surface
/// or undo the sandbox. They fail with EPERM, so tools degrade instead of
/// crashing (a few tools fall back cleanly, e.g. glibc without io_uring).
#[must_use]
pub fn denied_syscalls() -> Vec<(i64, &'static str)> {
    vec![
        (libc::SYS_bpf, "bpf"),
        (libc::SYS_keyctl, "keyctl"),
        (libc::SYS_add_key, "add_key"),
        (libc::SYS_request_key, "request_key"),
        (libc::SYS_mount, "mount"),
        (libc::SYS_umount2, "umount2"),
        (libc::SYS_pivot_root, "pivot_root"),
        (libc::SYS_move_mount, "move_mount"),
        (libc::SYS_open_tree, "open_tree"),
        (libc::SYS_fsopen, "fsopen"),
        (libc::SYS_fsmount, "fsmount"),
        (libc::SYS_fspick, "fspick"),
        (libc::SYS_mount_setattr, "mount_setattr"),
        (libc::SYS_unshare, "unshare"),
        (libc::SYS_setns, "setns"),
        (libc::SYS_ptrace, "ptrace"),
        (libc::SYS_process_vm_readv, "process_vm_readv"),
        (libc::SYS_process_vm_writev, "process_vm_writev"),
        (libc::SYS_kexec_load, "kexec_load"),
        (libc::SYS_kexec_file_load, "kexec_file_load"),
        (libc::SYS_init_module, "init_module"),
        (libc::SYS_finit_module, "finit_module"),
        (libc::SYS_delete_module, "delete_module"),
        (libc::SYS_io_uring_setup, "io_uring_setup"),
        (libc::SYS_io_uring_enter, "io_uring_enter"),
        (libc::SYS_io_uring_register, "io_uring_register"),
        (libc::SYS_userfaultfd, "userfaultfd"),
        (libc::SYS_perf_event_open, "perf_event_open"),
        (libc::SYS_open_by_handle_at, "open_by_handle_at"),
        (libc::SYS_name_to_handle_at, "name_to_handle_at"),
        (libc::SYS_fanotify_init, "fanotify_init"),
        (libc::SYS_swapon, "swapon"),
        (libc::SYS_swapoff, "swapoff"),
        (libc::SYS_reboot, "reboot"),
        (libc::SYS_acct, "acct"),
        (libc::SYS_quotactl, "quotactl"),
        (libc::SYS_iopl, "iopl"),
        (libc::SYS_ioperm, "ioperm"),
        (libc::SYS_syslog, "syslog"),
        (libc::SYS_settimeofday, "settimeofday"),
        (libc::SYS_clock_settime, "clock_settime"),
        (libc::SYS_clock_adjtime, "clock_adjtime"),
        (libc::SYS_adjtimex, "adjtimex"),
    ]
}

const CLONE_NS_FLAGS: &[u64] = &[
    libc::CLONE_NEWUSER as u64,
    libc::CLONE_NEWNS as u64,
    libc::CLONE_NEWNET as u64,
    libc::CLONE_NEWPID as u64,
    libc::CLONE_NEWIPC as u64,
    libc::CLONE_NEWUTS as u64,
    libc::CLONE_NEWCGROUP as u64,
];

/// Install the seccomp filters. Must run after Landlock (Landlock sets
/// `no_new_privs`, which seccomp needs for an unprivileged process).
///
/// # Errors
/// Fails if a filter can't be compiled or installed.
pub fn apply_seccomp() -> anyhow::Result<()> {
    let arch =
        TargetArch::try_from(std::env::consts::ARCH).map_err(|e| anyhow::anyhow!("{e:?}"))?;

    // Filter 1: denied syscalls, and clone() asking for new namespaces → EPERM.
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = denied_syscalls()
        .into_iter()
        .map(|(nr, _)| (nr, vec![]))
        .collect();
    let clone_rules = CLONE_NS_FLAGS
        .iter()
        .map(|&flag| {
            SeccompRule::new(vec![SeccompCondition::new(
                0,
                SeccompCmpArgLen::Qword,
                SeccompCmpOp::MaskedEq(flag),
                flag,
            )?])
        })
        .collect::<Result<Vec<_>, _>>()?;
    rules.insert(libc::SYS_clone, clone_rules);
    let deny: BpfProgram = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )?
    .try_into()?;

    // Filter 2: clone3 → ENOSYS. Its flags live in a struct seccomp can't
    // inspect, so make libc fall back to clone(), which filter 1 checks.
    let clone3: BpfProgram = SeccompFilter::new(
        BTreeMap::from([(libc::SYS_clone3, vec![])]),
        SeccompAction::Allow,
        SeccompAction::Errno(libc::ENOSYS as u32),
        arch,
    )?
    .try_into()?;

    seccompiler::apply_filter(&deny)?;
    seccompiler::apply_filter(&clone3)?;
    Ok(())
}
