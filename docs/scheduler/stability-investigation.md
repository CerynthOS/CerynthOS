# Runnable-task/watchdog stability investigation

Latest validated outcome: the preparation-before-attachment candidate passed the strengthened cold-start/burst/idle gates, all five shadow adaptive phases and all 90 matrix measurements. Final runtime is healthy Balanced with sched_ext enabled; temporary permissions and validation workloads are removed. [Final report and evidence](../benchmarks/validated-candidate.md).

## Initial finding (historical)

The inverted stop predicate in the user-ring-buffer drain callback is a confirmed API-contract error, but live testing now demonstrates that correcting it does not resolve the watchdog failure. The remaining cause is unresolved. The strongest new observation is scheduler uninterruptible sleep (`D`) immediately before an idle-soak watchdog; the blocking syscall/kernel stack has not yet been captured. No watchdog timeout, CPU-selection strategy, or scheduling-profile policy has been changed to hide failures.

The failed source used:

```c
return !!scx_bpf_dispatch_nr_slots();
```

The kernel's `bpf_user_ringbuf_drain` contract is **0: continue; 1: stop**. The sched_ext kfunc returns the **remaining** dispatch capacity. Therefore the old callback stops when it still has room, and continues when capacity has been exhausted. The correction is:

```c
return scx_bpf_dispatch_nr_slots() == 0;
```

| Remaining slots after dispatch | Old result | Correct result |
| --- | --- | --- |
| 31 | 1: stop early | 0: continue |
| 0 | 0: continue past capacity | 1: stop |

Primary references: [kernel BPF helper contract](https://github.com/torvalds/linux/blob/master/include/uapi/linux/bpf.h), [sched_ext dispatch-slot implementation](https://github.com/torvalds/linux/blob/v6.16/kernel/sched/ext.c).

## Evidence and chronology

The current boot's earliest saved watchdog failure is 24 October 2026 at 17:30:07.359838 UTC (23:00:07 IST), before the resumed Interactive burst experiment. Later failures occurred at 18:19:05, 18:22:09 and 18:23:07 UTC. The last followed rollback to Balanced. This is not a startup-timeout-only or Interactive-only problem.

The kernel dumps show runnable tasks waiting without a DSQ while several CPUs are idle. The dispatch path can dequeue up to 64 tasks in Rust and submit them to the outgoing ring. Its pending-work check inspects the incoming ring and user pending count, not outgoing records still waiting to drain. Stopping outgoing draining after one record can therefore leave runnable work without timely dispatch opportunities. This matches the observed forward-progress failure, but the complete causal chain is not yet established by a live before/after reproducer.

The actual VM source and dependency at `67be0435c454dc8984a11064dc8b9c6476c46b6d` contain the inverted predicate. The dependency's `RustLandBuilder` also copies its templates over the project-owned C/interface/bridge files during build. A direct edit would be overwritten on rebuild. The build now uses `scx_cargo::BpfBuilder` directly against the reviewed repository-owned files. Before/after hashes verify their preservation.

## Minimal fix and diagnostics

- The production drain callback is in `schedulers/cerynth-scx/dispatch.h`; the BPF program and host regression include the same callback.
- The callback stops only when no dispatch slots remain.
- The build compiles owned sources rather than replacing them with dependency templates.
- Diagnostic counters record user dequeues/submissions, error counts, nonempty batches, kernel dispatch/cancellation counters, queue sizes, a loop-progress sequence, PID and heartbeat. They do not influence scheduling decisions.
- Stopped status includes optional `exit_error` on error return. A forced kill or error stop is not accepted as a clean transition, even if kernel journal delivery is delayed.

The audit also identified that the Rust loop discards a dequeued task if outgoing reservation fails. No corresponding dispatch/dequeue error appears in the saved service logs. This remains a secondary risk, not an established cause of these runs. The targeted monitor fails on any transport error counter so a recurrence will be collected instead of silently accepted. CPU selection, affinity fallbacks, wakeup kicks, generation cancellation, task ordering and slice settings are unchanged in this first candidate.

## Deterministic regression

`tools/test_dispatch_progress.py` compiles the production callback with kernel-contract stubs and checks:

- A 64-task burst fills 32 available slots in the first drain and completes in the next drain.
- A one-slot dispatch stops before consuming a second task.
- Repeated 257-task bursts deliver every task exactly once over 100 rounds.
- Reintroducing the old predicate makes the same harness fail.

This proves the callback contract defect, not whole-kernel scheduler stability.

## Live reproducer and progression

`tools/cerynth-stability.py` transitions through Balanced, Interactive, Performance, Background, Interactive and Balanced. Each profile runs `tests/workloads/scheduler-burst.py`: 32 synchronized runnable workers, 20 ms CPU work per worker per burst, then 100 ms idle between bursts. Every worker must finish every burst.

The monitor checks actual kernel state, non-zombie process identity, same PID throughout a phase, current-process status/stats heartbeats, loop progress, worker progress/completion, and new kernel watchdog events. It records observations and journal cursors. A watchdog, unclean stop, unexpected disablement, failed process or lost progress aborts; it does not restart or advance to the next profile after failure.

The driver first runs a one-round smoke with 5-second workloads and 5-second idle soak. It then runs three rounds with 15-second workloads and a 60-second idle soak. Both results must pass and match the installed binary hash and running kernel. Only then does it run shadow adaptive evaluation and a fresh 90-measurement matrix with three repetitions per profile/workload. It does not reuse earlier-version measurements.

Kernel events are monitored during workloads and control operations, including shutdown/profile transitions. Unclean stop records block reattachment even if journald has not yet delivered the watchdog message.

## Recovery and permissions

The revised driver grants only the seven literal status/start/stop/profile commands. The rule is removed and `visudo -c` checked after each phase. Workloads run as the ordinary account in separate systemd cgroups; cleanup drains those groups before final permission revocation.

On failure, it captures kernel/daemon/runtime evidence, stops Cerynth to verified Linux fallback, and restores the saved files **without starting the old scheduler**. There is no automatic reattachment after a watchdog failure. Failed or unverified cleanup remains a nonzero result. SIGKILL and power loss cannot execute shell traps.

## First candidate identity and initial validation

The first frozen candidate source version is `464811a7eadb9eb2808e23b609e581694fac878c7650ad9391f85fd850301ea7`, based on repository commit `50839403176e2f868416720d965a46a8868c48ec`. Its manifest records owned sources, dependency sources/revision, Cargo lockfile, toolchain, kernel/BTF identity and binary hashes. The driver rejects changed source/binary identities before installation.

- cerynth-scx SHA-256: `47b48f813fd2773ed13a3d96f6fee48fbf4240d8859444172b1412ac44bd51c8`.
- cerynthd SHA-256: `dc1962ff9f03bc29f64cdbb7309e3c1cda0cb2fccd2ea8946c94a1947c47ae92`.
- Ubuntu release workspace regressions: 104 passed, 3 ignored.
- Ubuntu Python/C regressions: 23 passed.
- The new 32-worker workload completes under Linux fallback: all workers completed eight synchronized bursts in the baseline test.
- Owned BPF source hashes remain unchanged by compilation.
- These were the pre-launch checks. Subsequent live results below supersede the former pending status; the candidate failed stability.


## Live targeted results and diagnostic follow-up

Session `cerynth-validation-20261024T204743Z-37623` passed smoke and all 18 sustained burst/profile phases, then failed during the final Balanced idle soak. The last observations show PID 39494 changing from `R` to `D`, with an unchanged loop progress sequence of 12778. Status subsequently records `runnable task stall (watchdog failed to check in for 5.041s)`. Recorded dispatch, dequeue, failed-dispatch and cancellation counters are zero. This does not prove that heartbeat/statistics file I/O caused the stall, nor that all queue behavior is correct. A kernel stack/syscall observation is needed before the next scheduler code change.

A second external launch, `cerynth-validation-20261024T205647Z-39834`, used the identical binary and passed its 18 phases plus 60-second idle soak. That demonstrates intermittency, not repair. It was halted because the same binary had already failed. Interrupting the adaptive process exposed a driver defect: systemd can classify SIGINT as clean, allowing the matrix to begin. The matrix process was then killed and root cleanup completed. Neither session supplies a completed adaptive evaluation or matrix.

Both session cleanup records report workloads drained, original baseline unchanged, temporary rule absent, and previous runtime files restored with Linux default scheduling. Independent SSH checks confirm kernel state `disabled`, no validation units, and rule absence. Their full evidence is preserved locally in `artifacts/stability-investigation/targeted-sessions.tar.gz` and its extracted session trees.

Follow-up tooling changes:

- Interruptions now exit failed, and each phase requires explicit complete-result evidence independently of the service exit status. A real unprivileged Linux subprocess interruption test verifies exit 1, retained failure evidence and no completion marker.
- A scheduler binary with a recorded targeted failure is quarantined from normal validation, even if a later retry passed. The quarantine check rejects this binary before installation.
- The only next live entrypoint is diagnostic-only. It cannot reach adaptive evaluation or the matrix. It runs 18 burst/profile phases followed by a 120-second idle soak and always restores previous runtime files with Linux scheduling afterward.
- A bounded root observer validates the scheduler executable and daemon parent, samples only that scheduler's `D` state, and records kernel stack, syscall and wait channel. It uses its own lowest FIFO priority so it can observe a failing sched_ext instance, with a 20/50 ms per-wake real-time CPU limit, 50 ms sleeps, a 900-second deadline, and parent-liveness exit. It changes no system-wide scheduler settings and has no scheduler-control commands.
- Cleanup stops the observer and workload cgroups, restores runtime files, removes staging and temporary rules, and runs `visudo -c` on both diagnostic success and failure.

The revised diagnostic source manifest is `db2151b26bbb9a7a9dd9f91ee86947257af0ca8823d616b8452159c98ca3afd1`; entrypoint SHA-256 is `92423d3cb0b5bb559b0da4b3c704f08211d2ac6ae27dc4f0854ee80900ddc780`. The scheduler binary remains `47b48f813fd2773ed13a3d96f6fee48fbf4240d8859444172b1412ac44bd51c8`. All 26 Ubuntu tooling tests pass, including the interruption test and mocked diagnostic cleanup; shell syntax checks pass. The privileged stack sampler has not yet been exercised. Local VM authentication for this diagnostic invocation is pending; no new elevated run is claimed.


## First diagnostic outcome and monitor precision correction

Authenticated session `cerynth-validation-20261024T212945Z-44138` completed all 18 burst/profile phases, then aborted during idle soak with `status heartbeat stale/future/missing`. This invocation did not capture a watchdog failure. Its stack sampler stayed alive and captured only BPF-link unregistration waits during profile transitions (`close` -> `bpf_scx_unreg` -> `kthread_flush_work`). Those stacks do not establish a blocking dispatch-loop operation or explain the earlier watchdog failure. In particular, the earlier `D` observation must not be treated as proof that heartbeat file I/O caused a stall.

The last observed heartbeat was integer `1792877723`, first seen at `1792877723985` ms. The last successful observation was at `1792877725925` ms; cleanup subsequently captured heartbeat `1792877726`. A check using the integer timestamp as an exact instant can reject a roughly 2.2-second-old update as over three seconds old. The producer truncates timestamps to whole seconds, so the health check now considers that one-second timestamp interval. The separate three-second loop-progress limit is unchanged. A regression covers accepted quantization, definite staleness and future timestamps. This is a monitoring correction, not a scheduler stability repair.

The revised sampler also captures stack/syscall/wait-channel when a scheduler's recorded loop progress has not advanced for 1.5 seconds, even in runnable/sleeping states. Samples include monotonic time, kernel state, loop sequence and process state. It retains the same root scope, runtime/CPU bounds and automatic cleanup.

All 27 tooling tests pass on Ubuntu. New diagnostic source manifest: `4c3a95a09f27539f1649a7cda0228536a91605fe042c143ec273332f40ffd412`. Scheduler binary and root entrypoint hashes remain unchanged. Normal validation rejects the binary based on the recorded failed runs. A second diagnostic invocation requires local authentication and has not yet been claimed executed.

The first diagnostic cleanup reports exit 1, workloads drained, original baseline unchanged, previous runtime files restored, and temporary rule absent. Independent checks confirm kernel sched_ext disabled, no validation units loaded, and `/etc/sudoers.d/cerynth-validation` absent. Root `visudo -c` saved `/etc/sudoers: parsed OK`. The complete session is archived in `artifacts/stability-investigation/diagnostic-session.tar.gz` and its extracted session directory. No adaptive evaluation or matrix was launched in this diagnostic session.

## Reproduced file-open stall and reporting-process candidate

The second diagnostic reproduced the kernel watchdog after all 18 burst/profile phases. The sampler caught the active scheduler blocked in `open_last_lookups` / `openat` with write/create/truncate flags, while the kernel dump shows the policy process runnable within the same file-open path and CPUs idle. A filesystem dependency cycle is now the leading explanation; the exact lock owner remains inferred. This is substantially stronger evidence than the earlier detach-only `D` samples.

The minimal candidate moves all live status/statistics writes to a separate process through nonblocking datagrams, preserving parent heartbeat/progress timestamps and counting dropped observations. It also removes synchronous dispatch-loop logging. BPF is detached before reporter cleanup can wait. Policy, timeout and heartbeat clock-domain logic are unchanged. Tests pass: 108 Rust, 27 Ubuntu tooling; 3 Rust tests ignored. Live validation remains pending local authentication. The required sustained idle gate is now 120 seconds and the root stack observer stops before measurements. See the source identities, evidence, limitations and cleanup report retained locally at `artifacts/stability-investigation/reporting-isolation-report.md`.


## Reporting-isolation targeted pass

The frozen candidate passed authenticated session `cerynth-validation-20261024T221414Z-54510`: all 18 burst/profile phases, 32 workers per phase, 818 synchronized bursts and a 120-second idle soak, with no reported watchdog event. Raw health/worker evidence reconciles with the pass marker. Diagnostic cleanup restored previous runtime files and Linux scheduling; independent checks verify file restoration, no remaining scheduler/reporter/observer processes or validation cgroup, absent temporary rule, valid saved sudoers check and unchanged baseline manifests. This was a `--diagnostic-only` invocation, so the full matrix/adaptive evaluation remain pending the normal driver launch. See the linked reporting-isolation report for evidence and limitations.


## Remaining startup exposure and next candidate

Full session `cerynth-validation-20261024T225410Z-59565` passed both gates and shadow adaptive evaluation, then the kernel watchdog fired while starting Balanced after Linux-default CPU stress. Six measurements completed, none with three repetitions per combination. The initialization path attached BPF before ring-buffer creation and memory/procfs protection. The next candidate moves preparation before attachment and guards cleanup on failure, adds three Linux CPU-load/cold-start cycles to the gate, discovers scheduler children before heartbeat publication, and quarantines matrix/adaptive failures. Tests: 111 Rust passed, 3 ignored; 32 Ubuntu tooling passed. New candidate has not yet been installed. See the current report for exact identities and evidence. Runtime is independently verified in Linux fallback with restored files and no temporary rule.


## Final validation passed

Session `cerynth-validation-20261024T233703Z-68399` completed smoke, three Linux CPU-load/cold-start cycles, 18 sustained burst/profile phases, 120-second idle soak, the five-phase shadow sequence and all 90 fresh matrix measurements. Raw results reconcile into 30 groups of three repetitions. Independent process sampling covers all 90 windows and post-cleanup checks confirm live Balanced scheduling, fresh heartbeats, advancing progress and the installed frozen hashes. No new watchdog event is recorded. Cleanup exited 0, drained all validation units/cgroups, removed staging and the temporary rule, validated sudoers and verified the allocator setting. All 202 original baseline files are unchanged. Performance results do not show a general throughput advantage over Linux in this VM; shadow results do not validate automatic adaptation. See the final report linked above for measurements, limitations and reproducible audits. Nothing was committed or pushed.
