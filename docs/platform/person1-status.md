# Person 1 platform implementation and acceptance

This work covers the Person 1 responsibilities in the supplied master plan.
No numbered Person 1 task list was present in the repository or supplied in
the conversation. This report therefore records concrete deliverables and
limits; it does not certify completion of an unknown checklist or the entire
44-week OS roadmap. The weekly plan and Person 3 task ledger are unchanged.

## Implemented and exercised

| Area | Deliverable | Evidence |
| --- | --- | --- |
| Baseline preservation | Frozen scheduler source/dependency manifest; algorithm unchanged | `tools/platform_inventory.py` and `docs/contracts/platform-baseline-v1.json` |
| Kernel foundation | Pinned-source checks, cgroup/pressure configuration, isolated x86-64 VM profile | `scripts/vm/build-platform-kernel.sh`; completed Linux 7.1 build |
| Boot and scheduler recovery | Four profiles, contention progress, heartbeat identity, crash/freeze/dispatch/reporting/SysRq faults | Real KVM guest; raw console and completion markers |
| Runtime ownership | Endpoint lock, peer checks, bounded framing, concurrency cap and SIGTERM cleanup | Real daemon/CLI integration tests with a fake scheduler |
| Failed profile changes | Restore prior healthy profile; pause adaptation; report rollback failure accurately | Healthy rollback and failed-rollback regressions |
| Persistent configuration/state | First-boot settings, atomic fsync/rename, safe corrupt-state recovery | Concurrent-reader and symlink tests; staged release binaries |
| CPU/RAM resource hierarchy | Existing pinned Person 1 API consumed through the single systemd writer | Three descendant levels, actual CPU quotas/memory limits, cancellation and SIGKILL recovery |
| Accounting and pressure | Descendant PIDs/memory/CPU, cgroup PSI, machine pressure and process creation rate | Live hierarchy test and telemetry compatibility regression |
| Responsiveness | Bounded CPU contention with owner IPC and native Qt observations | Synthetic workload; offscreen rendering, not physical compositor latency |
| Packaging | Existing runtime layout, reproducible archive, upgrade config preservation, checksums, dependency inventory | Two identical payload builds and actual staged startup/recovery |
| Reproducibility/CI | Deterministic guest initramfs, build manifests, pinned manual kernel workflow, retained failure logs | `tools/qualify_platform.py`; `.github/workflows/platform-kernel.yml` |

Published measurements and evidence digests are in
[`person1-platform-validation.md`](../benchmarks/person1-platform-validation.md).
Build/reproduction instructions are in [`kernel/build.md`](../kernel/build.md).

## Enforcement and support matrix

| Control | Implemented boundary | Qualified scope |
| --- | --- | --- |
| CPU quota | systemd/cgroup v2 bandwidth | Real nested owner hierarchy and Linux-default VM throttling |
| CPU weight | Configured proportional preference | Value readback; no sched_ext weight guarantee |
| RAM high/max | cgroup v2 thresholds/hard limit | Readback, descendant accounting and contained guest OOM; no claim that logical pause frees RAM |
| Process count | cgroup v2 pids limit | Guest fork denied with kernel event; runtime process cleanup |
| Swap | Kernel interface | VM `memory.swap.max=0` readback; no active-swap workload or per-session API extension |
| I/O weight | Existing platform budget contract and kernel interface | VM readback; device fairness not benchmarked; session adapter does not request it |
| I/O throughput | Kernel configuration | No managed per-device throughput API or live device benchmark in this change |
| CPU affinity | Hardware/allowed-CPU inventory | No managed per-session cpuset API extension in this change |
| Disk capacity | Filesystem-dependent | No enforced per-session filesystem quota |
| GPU | Separate Person 3 subsystem | No new hardware partitioning guarantee |

The production resource hierarchy continues refusing custom sched_ext CPU
policy until its quota/weight semantics are separately qualified. The VM's
cgroup limit checks run under Linux default scheduling before scheduler
fault tests. These are intentionally distinct experiments.

## Acceptance still required

The following work is not represented as completed:

- A complete numbered Person 1 checklist and its task-by-task reconciliation.
- New session-aware sched_ext policies, cross-scheduler fairness comparisons,
  and performance claims supported by repeated representative workloads.
- Managed API extensions for per-session swap, affinity, device throughput
  and filesystem quotas where the storage platform can enforce them.
- A complete desktop distribution image, installation and upgrade validation,
  bootloader/rollback design, release signing and a reviewed license/SBOM set.
- Physical-machine boot, display/GPU drivers, hardware compatibility,
  power-loss/reboot recovery and physical desktop responsiveness qualification.
- Coordinated interface acceptance by Persons 1, 2 and 4 and release review.

The kernel test appliance and development runtime archive are useful tested
artifacts. They are not a public-alpha distribution or a signed release.
