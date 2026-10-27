# CerynthOS Person 4 — final validation passed

The repaired runtime completed all required **90 measurements**: five profiles × six workloads × three repetitions, plus the five-phase shadow adaptive evaluation. The strengthened startup/burst/idle gates passed and no new watchdog event was recorded in the successful session. At the end of this validation session, independent checks confirmed Balanced scheduling, enabled kernel sched_ext, fresh heartbeats and continued scheduler progress. Adaptation was paused, with shadow recommendations recorded.

Session identifier: `cerynth-validation-20261024T233703Z-68399`. Identifiers and raw timestamps are preserved from the VM; they are not converted into local-time performance claims.

## Delivered work

- `cerynth-policy-report` accepts `--input` and recursive `--directory`, without a hard-coded experiment directory, and handles live records without an actual-profile field.
- `benchmarks/matrix.sh` runs every profile/workload combination three times, preserving raw observations, metadata, workload logs, summaries and provenance. All 90 measurements in this report are new; no older-version results are mixed in.
- `benchmarks/adaptive.sh` records the required idle → interactive → CPU → mixed → idle sequence, recommendations, actual profiles, confidence/reasons, scheduler counters and resource metrics. Missing audit counters remain unavailable.
- `tools/cerynth-observe` provides the live scheduler/profile/recommendation/reason view. The successful driver also saved a live JSON observation after measurements.
- Runtime checks reject disabled sched_ext, stale/prior-process heartbeats, failed scheduler processes and bounded startup failures. The scheduler publishes stopped/error status on watchdog/BPF exit.
- Stability repairs correct the dispatch drain stop predicate, preserve repository-owned BPF sources during build, isolate filesystem reporting in a separate process using nonblocking IPC, and finish ring-buffer/memory preparation before attaching BPF. Setup/attachment failures restore allocator-related state.
- The validation driver preserves baselines, quarantines failed binaries, requires complete-result evidence independently of service exit status, drains workload cgroups, restores Linux fallback on failure, and removes temporary privileges/staging files.

## Tests and live gates

| Check | Result |
|---|---|
| Release workspace tests | 111 passed, 0 failed, 3 ignored |
| Ubuntu tooling/C regression tests | 32 passed |
| Smoke | Six profile phases, 32 workers, Linux-load cold start and short idle check passed |
| Sustained stability | Three 30-second Linux-default CPU-load → cold-start cycles, 18 profile/burst phases and 120-second idle soak passed |
| Runnable-worker completion | All 32 workers completed every synchronized burst: 93 smoke bursts and 810 sustained bursts |
| Adaptive sequence | All five phases completed in shadow mode |
| Matrix | 90/90 complete; 30/30 profile/workload groups have three repetitions |
| Independent process coverage | All 90 measurement windows covered; 1,850 healthy Cerynth process samples |
| Source integrity | No owned source or dependency hash changed during benchmarking |
| Original baseline integrity | All 202 original files match the before/after manifests |

Regressions cover heartbeat/process identity and freshness, scheduler process failure, kernel disablement, startup timeout cleanup, watchdog/no-reattach behavior, repeated profile transitions, dispatch capacity, reporter backpressure/termination, preparation-before-attachment, cold-start baseline failure, exact control arguments, completion evidence, quarantine and cleanup/recovery paths. Tests using mocked system commands are not presented as live kernel fault injection.

## Performance comparisons

These are descriptive results from a four-CPU Ubuntu **aarch64 VM**, kernel **7.0.0-29-generic**. They do not establish behavior on physical hardware or a custom Cerynth kernel. Profile order rotates between repetitions; three repetitions provide limited precision and are not randomized large-sample experiments.

CPU utilization below is mean ± sample standard deviation across the three runs, in percent of total VM CPU capacity. It includes the workload, observability and background activity. **CPU utilization is a resource measure, not a speed or latency score.**

| Workload | Linux default | Balanced | Interactive | Performance | Background |
|---|---:|---:|---:|---:|---:|
| idle | 0.46 ± 0.05 | 0.64 ± 0.03 | 0.64 ± 0.03 | 0.66 ± 0.01 | 0.63 ± 0.03 |
| cpu-stress | 99.15 ± 0.07 | 99.42 ± 0.09 | 99.48 ± 0.10 | 99.13 ± 0.05 | 98.75 ± 0.18 |
| interactive-stress | 75.35 ± 0.18 | 82.01 ± 1.43 | 79.76 ± 1.82 | 84.82 ± 0.78 | 82.85 ± 0.85 |
| burst | 58.56 ± 0.19 | 59.35 ± 0.20 | 58.86 ± 1.36 | 59.36 ± 0.91 | 59.80 ± 1.09 |
| compile | 47.88 ± 1.44 | 44.52 ± 4.20 | 51.67 ± 2.38 | 50.78 ± 2.79 | 51.94 ± 1.62 |
| mixed | 26.19 ± 0.02 | 27.98 ± 0.28 | 28.04 ± 0.49 | 27.70 ± 0.23 | 27.45 ± 0.49 |

Synthetic throughput and build ratios below are paired by repetition against Linux default, then averaged; ± is the sample standard deviation of the three ratios, not a confidence interval. Linux is 1.000; higher ratios mean more bogo operations/second or shorter build time, respectively.

| Profile | CPU-stress throughput / Linux | Mixed throughput / Linux | Linux build time / profile build time |
|---|---:|---:|---:|
| Balanced | 0.996 ± 0.015 | 1.001 ± 0.005 | 0.994 ± 0.178 |
| Interactive | 0.987 ± 0.012 | 1.002 ± 0.005 | 0.931 ± 0.085 |
| Performance | 0.990 ± 0.010 | 1.000 ± 0.005 | 1.005 ± 0.113 |
| Background | 0.997 ± 0.008 | 1.001 ± 0.001 | 0.988 ± 0.058 |

The CPU-stress means are approximately 0.987–0.997× Linux, while mixed synthetic throughput is approximately 1.000–1.002×. These results do **not** demonstrate a general throughput advantage. Warm-build ratios have substantial run-to-run variation and do not support a consistent speedup claim.

The compile workload cleans the telemetry package and runs a release workspace build with an otherwise warm dependency cache. Its ratios use the shell's recorded `real` build time, not the one-second sampling interval. This is not a cold full-workspace/kernel build. Interactive iteration and burst counters are timer-bound proxies, not measured response latency; no latency improvement is claimed. Diagnostic stack sampling stopped before adaptive and matrix measurements.

## Shadow adaptive evaluation

The actual scheduler remained Balanced throughout this sequence, with zero observed profile changes. Recommendations changed with workload pressure, but were not applied. Automatic adaptation/canary switching and rollback behavior were not exercised. The adaptation-audit producer and starvation-save counter remain unavailable; unavailable rollback/starvation values are not interpreted as zero.

| Phase | Elapsed seconds | CPU utilization % | Recommendations observed |
|---|---:|---:|---|
| idle | 30.60 | 0.62 | balanced: 30 samples |
| interactive-stress | 60.66 | 80.46 | balanced: 60 samples |
| cpu-stress | 60.72 | 98.95 | balanced: 3 samples, performance: 57 samples |
| mixed | 60.31 | 27.56 | performance: 2 samples, interactive: 58 samples |
| idle | 30.49 | 0.64 | interactive: 1 sample, balanced: 29 samples |

Confidence and reasons are retained for every adaptive observation. CPU pressure produced Performance recommendations in 57/60 samples; mixed load produced Interactive recommendations in 58/60 samples. The remaining samples show transition/hysteresis behavior. Shadow evaluation and the complete matrix therefore pass within their recorded scope; active automatic adaptation remains outside this validation.

## Failures, repairs and recovery history

Earlier candidates failed real kernel watchdog checks. The original dispatch-callback bug was a confirmed API-contract error, but fixing it alone was insufficient. A later diagnostic captured the active scheduler blocked in file creation while runnable tasks awaited scheduling, supporting a filesystem dependency-cycle hypothesis. The exact lock owner was not directly proven.

Reporting isolation passed targeted tests, then an earlier full run failed during Balanced startup after six matrix measurements. The startup path still attached BPF before ring-buffer creation and memory/procfs protection. The final candidate moves that potentially blocking preparation before scheduler ownership. The precise blocking operation in the earlier startup failure was not captured; that mechanism is inferred from the confirmed unsafe ordering. The final candidate passed the strengthened cold-start gates and complete new matrix without a recorded watchdog.

Failed runs, raw observations, kernel/service logs and rollback evidence remain in separate archived session trees. Earlier recovery restored previous binaries/configuration and verified Linux fallback instead of repeatedly reattaching a failing scheduler. None of the old partial results contribute to the comparisons above.

## Reproducing the dependency and checks

The dependency revision used by the validated candidate is recorded in [`third_party/scx.revision`](../../third_party/scx.revision). The upstream tree itself is ignored by the parent repository. Before building, obtain that exact revision:

```bash
git clone --filter=blob:none https://github.com/sched-ext/scx.git third_party/scx
git -C third_party/scx switch --detach "$(cat third_party/scx.revision)"
```

For an existing dependency checkout, inspect and preserve its changes before switching revisions. Clone only when the directory does not already exist.

On the prepared Ubuntu development environment:

```bash
CARGO_BUILD_JOBS=2 cargo test --release --workspace
python3 -m unittest discover -s tools -p 'test_*.py'
```

The interruption regression requires Linux fallback and skips while sched_ext is enabled. A later rerun on the healthy VM completed 111 Rust tests (3 ignored), and 31 tooling tests with that one test skipped. The complete 32-tooling-test pass above occurred before the live run.

The privileged [`run-vm-validation.sh`](../../tools/run-vm-validation.sh) requires explicit `--source`, `--run-user` and `--baseline` arguments. Supply existing absolute paths for the source workspace and preserved baseline, and a non-root workload account. Add `--diagnostic-only` for targeted checks. It requires a local frozen manifest and the documented Cerynth service layout; it is not a general installer. Machine-specific configuration and generated evidence remain local. The driver configuration interface was corrected after validation; the scheduler implementation is unchanged, and this interface revision has unprivileged regression coverage but has not undergone a new privileged VM run.

## Candidate identity and local evidence

- Base commit: `50839403176e2f868416720d965a46a8868c48ec`.
- SCX dependency: `67be0435c454dc8984a11064dc8b9c6476c46b6d`.
- Frozen source identity: `c8e76039e5c6ddd89d49940402679433a8792383ca66969a45ee4615e4c32cd2`.
- Installed scheduler SHA-256: `4199c8c376374fee6910422656f38fd8fa96b17b94dcf32f8405a147c8d286aa`.
- Installed daemon SHA-256: `dc1962ff9f03bc29f64cdbb7309e3c1cda0cb2fccd2ea8946c94a1947c47ae92`.

The local evidence bundle remains under ignored `artifacts/stability-investigation/`: `final-success-session.tar.gz`, `initialization-candidate.tar.gz`, `final-independent-checks.json`, `final-matrix-audit.json`, `final-health-audit.json`, `final-adaptive-audit.json`, and the reproducible audit scripts. These archives, raw measurements and binaries are intentionally not part of Git. The detailed local report remains at `artifacts/vm-validation/final-success/final-report.md`.

Cleanup exited zero. Independent checks verified removal of the temporary sudo rule, inactive validation units with empty cgroups, absent staging files and observer process, expected allocator setting, and unchanged original baseline hashes. Root cleanup's `visudo -c` output was parsed OK; subsequent noninteractive sudo status required authentication. The corrected runtime was left healthy on Balanced with adaptation paused. These are session-specific observations, not a claim about a future machine's state.
