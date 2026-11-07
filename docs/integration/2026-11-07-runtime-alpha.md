# CerynthOS Full Integration Test: Report

> Evidence files referenced below (`*.txt`, `vm/...`) were written to the gitignored `artifacts/full-integration/` directory of the tested checkout and are not committed.

**Result: PASS.** CerynthOS Runtime Alpha is integrated.

| | |
|---|---|
| Date | 2026-11-07 |
| Commit | `55b553e` (main, CI green), clean tree, frozen for the whole run |
| Host | Arch Linux, 8 cores, KVM |
| Test machine | QEMU/KVM VM, Ubuntu 24.04 userspace, 4 vCPU, 6 GB |
| Kernel under test | **Custom Cerynth kernel `7.1.0+`** (pinned `8cd9520d`), built from `kernel/config/base.config` |
| scx dependency | `67be0435…`, matches `third_party/scx.revision` |
| Binaries | Same SHA-256 on host and in the VM (`binary-hashes.txt`). The stability tool independently recorded the scheduler hash `b0f9a4e7…` |

## Scorecard

| Integration area | Result | Evidence |
|---|---|---|
| Clean workspace build | ✅ | `build-release.log` |
| Rust test suite | ✅ 185 passed, 0 failed | `test-release.log` |
| Tooling tests | ✅ 34 OK | `tooling.log` |
| `cargo fmt --check` | ✅ | `fmt.log` |
| Custom Cerynth kernel boot | ✅ 7.1.0+ | `platform-gate.txt` |
| BTF | ✅ | `platform-gate.txt` |
| sched_ext interface | ✅ | `platform-gate.txt` |
| Normal Linux userspace | ✅ 8/8 | `linux-sanity.txt` |
| Runtime installer (incl. policy service) | ✅ | `provision.log` |
| `cerynthd` systemd startup | ✅ | `runtime-up.txt` |
| Policy service startup | ✅ | `runtime-up.txt` |
| Scheduler auto-start | ✅ | `runtime-up.txt` |
| Balanced / Interactive / Performance / Background | ✅ ✅ ✅ ✅ | `profile-switches.txt` |
| Scheduler heartbeat under load | ✅ 0–1 s | `heartbeat-progress.txt` |
| Scheduler progress counter | ✅ | `heartbeat-progress.txt` |
| Error counters zero | ✅ | throughout |
| `cerynth-observe` (text + JSON) | ✅ | `observer.txt` |
| Shadow recommendations follow the workload | ✅ | `shadow-policy.tsv` |
| Shadow does **not** auto-switch | ✅ same profile and PID across all samples | `shadow-policy.tsv` |
| Adaptive sequence | ✅ 5/5, 0 profile changes | `vm/adaptive/` |
| Smoke stability gate | ✅ 6/6 phases, 0 watchdog events | `vm/stability-smoke/` |
| Sustained stability gate | ✅ 18/18 phases, 3/3 Linux baselines, 0 watchdog events | `vm/stability-sustained/` |
| Normal stop → Linux fallback | ✅ | `fallback-manual.txt` |
| `kill -9` → Linux fallback | ✅ ~130 ms (fault suite) | `kill9.txt`, `vm/scx-faults/` |
| Scheduler restart after crash | ✅ new PID, exactly one process | `kill9.txt` |
| Reporter process does not leak | ✅ exits with its parent | `kill9.txt` |
| Policy-service crash recovery | ✅ systemd restart, scheduler unaffected | `policy-and-daemon-restart.txt` |
| Daemon restart and persistent state | ✅ profile kept, no duplicate schedulers | `policy-and-daemon-restart.txt` |
| Scheduler fault suite (`fault-test.sh`) | ✅ 9/9 scenarios, 220/220 checks | `vm/scx-faults/` |
| Lifecycle stress (`stress-lifecycle.sh`) | ✅ 25/25 iterations, 1,951/1,951 checks, fallback avg 136 ms | `vm/scx-lifecycle/` |
| Full reboot | ✅ 10/10 checks, nothing started by hand | `reboot.txt` |
| Benchmark matrix | ✅ 90/90 runs | `vm/benchmark-matrix/` |
| No new kernel watchdog/stall | ✅ see note 1 | `final-logs-and-cleanup.txt` |
| Final state | ✅ balanced, adaptation paused, 1 healthy scheduler, no failed units | `final-logs-and-cleanup.txt` |

## Deviations from the plan

1. **Laptop suspend during the first matrix attempt.** The host auto-suspended at 16:27 IST and resumed at 16:53. The frozen VM's clock jumped 1,596 s, and sched_ext ejected the scheduler (`runnable task stall`, `watchdog failed to check in for 1596.086s`). The host's suspend/resume timestamps line up with the kernel messages to the second, so this was an environment event, not a CerynthOS failure. Linux fell back safely. The matrix was re-run with `systemd-inhibit` blocking sleep. Evidence: `matrix-failure-evidence.txt`, `vm/benchmark-matrix-interrupted-by-host-suspend/`.
2. **Matrix run length 10 s instead of 30 s.** All 90 runs were kept (the harness requires ≥ 3 repeats), but each run was shortened to save time. The results are valid for pass/fail. For performance comparisons, re-run at 30 s.
3. **Kernel modules trimmed with `localmodconfig`.** Built-in options are unchanged apart from driver sub-options (wifi tracing, joysticks, KVM *host* features, etc.). The scheduler, sched_ext, BPF, cgroup and tracing configuration are the same as `base.config`. Modules went from 6,331 to the 51 the VM uses. Evidence: `kernel-config-tested.config`, `kernel-localmodconfig-removed-builtins.txt`.
4. **Strict `clippy -D warnings` still fails** (pedantic lints, vendored scx crates, 8 known leftovers). It isn't a CI gate.

## Findings and follow-ups

**Worth investigating**
- After the suspend-induced ejection, the scheduler process **exited without logging and without writing its "stopped" status**, so `scheduler.json` kept saying `running: true, healthy: true`. In `fault-test.sh heartbeat-freeze` (watchdog ejection of a frozen scheduler) it *did* write "stopped". The difference may be the whole-VM freeze. Owner: scheduler.
- The **interactive** workload never got an `interactive` recommendation in shadow mode (15/15 `balanced`), while the mixed workload did. Tune this before adaptation goes live. Owner: policy.

**Small fixes**
- `setup-base-vm.sh` writes the seed to `vm/generated/seed.iso`, but `run-base-vm.sh` reads `vm/cloud-init/seed.iso`.
- The plan's matrix command needs `sudo`: the daemon socket is `root:root 0660`.
- The `compile` workload needs `cargo` and `third_party/scx` inside the VM. Provisioning doesn't set these up.
- The kernel warns that writing `p->scx.slice` directly is deprecated. Move the vendored BPF code to `scx_bpf_task_set_slice()`.
- Binaries are built on the host (glibc 2.44) and run in the VM (glibc 2.39). This works today only because the binaries need ≤ 2.39. Build in the VM or in an Ubuntu container.
- `cerynthd` doesn't handle the scheduler's exit code 75 (restart request).
- `rollback()` / `verify_post_switch_health()` are not wired into the daemon.
- `cerynthctl` defines `REQUEST_TIMEOUT` but never uses it.
- There are two scx pin files (`third_party/scx.revision`, `schedulers/SCX_COMMIT`). Keep one.
- Consider a small `kernel/config/vm.config` fragment so VM kernels build in minutes.
- `clocksource: Watchdog remote CPU read timed out` appears under host load. This is normal for VMs; it was present from 3 s after boot, before the scheduler started.

## Benchmark summary (10 s runs, 3 repeats)

CPU-bound throughput (`stress-ng` bogo ops/s) is within noise across all schedulers: 8,927–8,991, with Linux at 8,948. Interactive iterations and burst completion are also in line with Linux. No profile shows a regression. As the report itself says, CPU % isn't a throughput score, so no performance claims are made from this run. Full table: `vm/benchmark-matrix/report.md`.

## Not tested in this stage (by design)

Automatic profile switching, canary adaptation, automatic rollback, AI command interface, desktop UI, installer ISO, physical hardware, ML/RL scheduler.
