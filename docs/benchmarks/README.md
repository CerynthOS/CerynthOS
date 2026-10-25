# Telemetry and benchmarks

Latest VM validation completed **90/90 measurements**, with three repetitions for all 30 profile/workload combinations, plus the five-phase shadow adaptive sequence. See the [final report, performance comparisons and cleanup evidence](validated-candidate.md). The results are specific to the Ubuntu aarch64 VM and do not establish a general speedup or active adaptation behavior.

This is the Person 4 (telemetry/benchmarks) track of the CerynthOS weekend
sprint. It provides the measurement layer the rest of the project (and
eventually MIMIR) will use to reason about scheduler behaviour: a
`SystemSnapshot` collector, a recorder binary, workload scripts, and a
benchmark runner that ties them together into reproducible, self-contained
artifact directories.

Nothing here assumes a working `cerynth-scx`/`cerynthd` yet — the `--profile`
flag on the benchmark runner is just a label used to organize output
directories and can be `linux-default`, the name of an upstream `scx`
scheduler you started manually, or a future Cerynth profile name.

## Telemetry crate (`crates/cerynth-telemetry`)

`SystemSnapshot` is a point-in-time reading of system and scheduler state,
built from `/proc` and `/sys`:

- CPU usage %, load averages (1/5/15m), runnable/total task counts
- context switches and interrupts (cumulative kernel counters)
- memory and swap usage
- the top 5 processes by CPU usage over the sampling interval
- current `sched_ext` state (`/sys/kernel/sched_ext/state`)
- current scheduler profile, read from `/var/lib/cerynth/state.json` if
  `cerynthd` has written one (`null` otherwise)

`ProcTelemetrySource` implements the `TelemetrySource` trait and is the only
source today. CPU-usage percentages require differencing against the
previous sample, so the first `collect()` call always reports `0.0` — the
recorder and benchmark runner both account for this by taking one throwaway
sample before recording.

### Recording telemetry directly

```bash
cargo run --release -p cerynth-telemetry --bin cerynth-telemetry-record -- \
  --interval-ms 1000 \
  --duration-seconds 60 \
  --output data/run.jsonl
```

Each line of the output file is one JSON-encoded `SystemSnapshot`.

## Workload scripts (`tests/workloads/`)

Each script takes a single argument, `<duration-seconds>`, and runs to
completion within roughly that time budget:

| Script | What it does |
|---|---|
| `idle-baseline.sh` | Sleeps; induces no load. Baseline for comparison. |
| `cpu-stress.sh` | Saturates all CPUs via `stress-ng --cpu $(nproc)`. |
| `interactive-stress.sh` | Background CPU load plus a foreground loop of short-lived processes, approximating an interactive session competing with a busy job. |
| `burst.sh` | Alternates short `stress-ng` bursts with idle pauses. |
| `compile.sh` | Runs `cargo build --workspace --release` as a real-world mixed workload. Duration argument is accepted but advisory only. |

Scripts that need `stress-ng` check for it first and print an install
command (`sudo apt install -y stress-ng`) instead of failing unhelpfully.

## Benchmark runner (`benchmarks/run.sh`)

```bash
./benchmarks/run.sh --profile <profile> --workload <workload> --duration <seconds>
```

Example:

```bash
./benchmarks/run.sh --profile linux-default --workload cpu-stress --duration 30
```

This builds the telemetry recorder in release mode, starts it in the
background, runs the requested workload script, and writes a self-contained
artifact directory:

```text
artifacts/benchmarks/<date>/<profile>/<workload>/
  metadata.json     # kernel, CPU, sched_ext state before/after, exit code, timing
  telemetry.jsonl   # one SystemSnapshot per line for the whole run
  telemetry.log     # stderr progress output from the recorder
  workload.log      # stdout+stderr from the workload script
  summary.json      # avg/max CPU%, avg load, context-switch/interrupt deltas
  summary.md         # the same summary, formatted for quick reading
```

`artifacts/` is gitignored — these are generated per-run and not meant to be
committed. Commit summaries or comparisons manually if you want to preserve
a specific baseline for the team.

### Comparing profiles

Run the same workload under different profiles/schedulers and diff the
`summary.json` files, e.g.:

```bash
./benchmarks/run.sh --profile linux-default --workload cpu-stress --duration 30
sudo ./target/release/scx_rustland &
./benchmarks/run.sh --profile scx-rustland --workload cpu-stress --duration 30
```

This establishes reproducible baselines only — no performance claims are
implied by these numbers alone.

## Sprint: complete matrix and adaptive evaluation

These tools require Python 3.9+, Bash, `stress-ng`, Cargo and a Linux VM with
a sched_ext-capable kernel, daemon configured for the **scx** backend, and `cerynthctl`
on PATH. Build the workspace before measurement. Run with an account authorized
to control the daemon. The runners never enable canary adaptation themselves.

```bash
./benchmarks/matrix.sh --plan
# Pause adaptation first; verify state.json says adaptation_enabled=false.
# The default runs 5 profiles × 6 workloads × 3 independent repetitions.
./benchmarks/matrix.sh --repeats 3 --duration 30
./benchmarks/adaptive.sh --plan
# Uses the currently configured shadow or explicitly enabled canary mode.
./benchmarks/adaptive.sh
./tools/cerynth-observe
./tools/cerynth-observe --once --json
```

The matrix tests `linux-default`, `balanced`, `interactive`, `performance`,
`background` against `idle`, `cpu-stress`, `interactive-stress`, `burst`,
`compile`, `mixed`. Repetition order rotates the starting profile to reduce
order bias. Each run has its own directory, so previous repeats cannot be
replaced by the next one. Existing experiment output directories are rejected;
use `--output artifacts/benchmark-matrix/run-2` for another experiment.

`artifacts/benchmark-matrix/` contains:

```text
plan.json
raw/<profile>/<workload>/<repeat>/
  metadata.json
  observations.jsonl
  workload.log
  adaptation-audit.jsonl
  summary.json
summaries/<profile>-<workload>.json
report.md
```

Reports give means, sample standard deviations, minima and maxima across at
least three independent runs. Elapsed time and raw workload throughput counters
remain available for interpretation. CPU utilization alone does not demonstrate
better performance. Compilation runs to completion, so its duration is advisory;
compare its elapsed time alongside resource metrics. The existing compile
workload cleans the telemetry crate before building the release workspace.

Every matrix sample validates the actual scheduler status, fresh heartbeat
(seconds since epoch, maximum age five seconds), profile and kernel sched_ext
state. Invalid state or workload failure aborts the experiment, preserves raw
evidence and writes `failure.json`; no complete report is claimed. On successful completion, the runner restores the original profile. In
stability-gated runs, a failure suppresses automatic reattachment; the privileged
driver captures evidence, restores files and verifies Linux fallback. Standalone
ungated runs retain their ordinary restoration behavior. SIGKILL or host loss
cannot execute cleanup. Do not enable
adaptation concurrently with a matrix run.

The adaptive sequence is fixed at idle 30s, interactive 60s, CPU stress 60s,
mixed 60s, idle 30s. Its `artifacts/adaptive-evaluation/raw/<phase>/` records
recommendations, confidence, reasons, actual profiles and changes, scheduler
status/stats, CPU usage, context switches, and process creations per second.
Audit events appended during a phase are copied to `adaptation-audit.jsonl`,
including rollback events. File rotation and truncation are handled. Existing
audit history is excluded. Missing audit files or scheduler stats are unavailable
evidence; this cannot establish that no rollbacks or scheduler errors occurred.
Use `--audit`, `--runtime-dir`, `--state`, `--ctl`, and `--interval` for local
paths or alternate installations. The observer also honors `CERYNTH_STATE`
and `CERYNTH_POLICY_PATH`. It never changes runtime state.

`tools/cerynth-observe` shows the kernel, sched_ext state, verified scheduler
health, active profile, heartbeat age, adaptation mode, recommendation,
confidence, reason, CPU use, runnable tasks, context switches, churn, dispatches,
starvation rescues and dispatch/dequeue errors. Missing or malformed files show
unavailable values. CPU/rate readings start after a baseline interval. Add
`tools/` to PATH to invoke it as `cerynth-observe`.

Scheduler stats are optional until Person 2 provides
`/run/cerynth/scheduler-stats.json`. The consumer uses `tasks_dispatched`,
`starvation_rescues`, `dispatch_errors`, `dequeue_errors` and preserves all other
fields in raw observations without changing the producer interface.

Policy reports accept a single JSONL or recursively discovered JSONL files:

```bash
cargo run -p cerynth-policy --bin cerynth-policy-report -- \
  --input artifacts/policy-decisions.jsonl
cargo run -p cerynth-policy --bin cerynth-policy-report -- \
  --directory artifacts/policy-replay/
```

The directory must contain policy records, not unrelated telemetry JSONL.
Both replay records and captured live recommendations are supported. Missing,
empty directories and malformed records fail explicitly.

Portable verification (does not run a scheduler):

```bash
python3 -m unittest discover -s tools -p 'test_*.py' -v
./benchmarks/matrix.sh --plan
./benchmarks/adaptive.sh --plan
cargo test -p cerynth-policy -p cerynth-telemetry
```

Live benchmark results must be collected inside Linux. Plans and fixture tests
are tooling validation, not measured performance or proof of safe adaptation.

### Resuming an interrupted matrix

```bash
./benchmarks/matrix.sh --resume --output artifacts/benchmark-matrix
```

Use the original duration, repeat count and sampling interval. Resume requires
an identical saved plan and verifies the successful run metadata and every raw
scheduler-health observation before reusing a result. Incomplete run directories
are moved to `failed-attempts/`; prior failure records are retained with a
`failure-before-resume-` prefix. A failed experiment never becomes a successful
report merely because it has some completed runs.

Linux VM testing also exposed two daemon integration issues: restart tests were
loading the machine's real configuration, and startup health accepted a disabled
sched_ext state or a different process's heartbeat. The tests now preserve their
isolated configuration across restart. Startup requires sched_ext `enabled`, a
fresh heartbeat from the tracked child with `running` and `healthy` true, and a
live child, within a bounded 15-second startup window. These checks do not change
scheduler profile behavior or the shared JSON field names.

Each measurement session also writes `measurement-session-*.json` with separate
lists of reused and newly measured runs. Final per-combination summaries include
`measurement_origins`, and the matrix report states the reused/new counts. Keep
the original matrix archive and daemon hashes when comparing sessions that span
a runtime-control update.

For continuation that must leave the original artifact tree untouched, use a
new output directory with the old matrix as read-only input:

```bash
./benchmarks/matrix.sh --reuse-from artifacts/old-matrix \
  --output artifacts/new-matrix
```

Only newly measured runs are written under the new output's `raw/`; combined
summaries count verified old measurements without copying or rewriting their
raw data. The privileged validation driver is `tools/run-vm-validation.sh`;
it runs workload phases as the ordinary user in separate systemd cgroups,
drains those cgroups before rollback and privilege removal, and saves explicit
cleanup results. Review the driver before executing it with sudo.
