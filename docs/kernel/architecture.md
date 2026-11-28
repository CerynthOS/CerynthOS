# Kernel and platform boundaries

CerynthOS uses Linux; the source revision and release are pinned in
`kernel/COMMIT` and `kernel/VERSION`. Kernel code stays in the ignored
`third_party/linux` checkout. Platform services, adapters and user interfaces
remain separate processes. Fetching or building a kernel does not install it
on the workstation.

| Component | Responsibility | Authority |
| --- | --- | --- |
| Linux `kernel/sched/ext.c` | sched_ext callbacks, watchdog and fallback | Kernel |
| Scheduler `main.bpf.c` | Enqueue and dispatch mechanisms | Verified BPF program |
| Scheduler Rust process | Scheduling policy and bounded dispatch | Privileged process |
| Scheduler reporter | Filesystem status/statistics publication | Separate child |
| `cerynthd` | Process ownership, profile changes, health and rollback | Trusted system service |
| systemd | Sole writer for managed resource hierarchies | System/user manager |
| `cerynth-agentd` | Session/process attribution and admission | Unprivileged owner service |
| Desktop | Inspection and authorized requests | Client |

Linux process creation and exit live in `kernel/fork.c` and `kernel/exit.c`;
the common scheduler lives in `kernel/sched/`. cgroup CPU, memory, process-count
and block-I/O controllers impose different boundaries. A logical session
pause is not memory reclamation. A CPU weight is a proportional preference,
whereas CPU bandwidth is a quota. Device scheduling determines whether an
I/O weight has an observable effect.

The dispatch process must not wait for filesystem reporting, page faults or
allocations that require the tasks it schedules to run. Preparation occurs
before attachment; the reporter handles filesystem output. The frozen
scheduler source is recorded in
[`platform-baseline-v1.json`](../contracts/platform-baseline-v1.json).
Supervisor improvements do not change that scheduling algorithm.

`cerynthd` owns its child rather than trusting a PID found in a status file.
It checks process identity, kernel state, heartbeat and profile before
accepting startup. A failed profile change attempts restoration of the prior
healthy profile and pauses adaptation. A failed restoration is reported as
failure. SIGTERM stops the owned scheduler; systemd also owns the service's
process cgroup. Abrupt termination of an unmanaged daemon requires explicit
process recovery; it is not an automatic rollback of external work.

`cerynthctl` uses versioned newline-framed Unix-socket messages. Both sides
bound input size and receive time. The daemon keeps an exclusive endpoint
lock, refuses replacement of unrelated files or active endpoints, validates
peer credentials, and caps concurrent connections. This control socket is
for the trusted owner/root; managed harnesses must not receive it.

The existing Person 1 resource contract is preserved verbatim in
`crates/cerynth-platform-resources`, with its source revision and file hashes
in `PROVENANCE.json`. Session management uses an adapter to that contract and
a single systemd writer. CPU quotas, memory limits and descendant accounting
are available through the opt-in hierarchy. sched_ext CPU-weight semantics
remain unqualified, so that hierarchy requires Linux's default scheduler.

BTF is a build artifact, not merely a configuration switch. The pre-existing
Linux 7.1 image booted and enforced cgroups but exposed a kfunc prototype
that the frozen loader rejected. A fresh build from the same pinned kernel
source allowed the unchanged scheduler to attach. The original artifact's
build failure mechanism has not been established. Changing a loader version
alone did not repair that artifact; no loader update is introduced here.
