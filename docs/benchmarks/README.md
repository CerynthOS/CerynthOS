# Telemetry and benchmarks

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
