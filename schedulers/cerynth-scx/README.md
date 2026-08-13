# cerynth-scx

CerynthOS's sched_ext scheduler. Built on `scx_rustland_core` — the BPF
plumbing (`main.bpf.c`, `bpf.rs`, `bpf_intf.rs`, `bpf_skel.rs`) is shared,
generic infrastructure; the Rust side in `src/` is the actual scheduling
*policy*.

## Module layout

- `main.rs` — entry point: CLI parsing, the dispatch loop, status-file
  lifecycle, signal handling.
- `profile.rs` — the four scheduling profiles and their time slices.
- `policy.rs` — pure, BPF-free task-ordering logic (`order_tasks`), unit
  tested directly with no kernel or root required.
- `error.rs` — typed errors for dequeue/dispatch failures.
- `status.rs` — atomic status-file writer (`/run/cerynth/scheduler.json`).

## Profiles

| Profile       | Time slice | Ordering                     | Use case                                   |
|---------------|-----------:|-------------------------------|---------------------------------------------|
| `balanced`    | 5 ms       | FIFO (arrival order)          | Default, general-purpose desktop use        |
| `interactive` | 2 ms       | Shortest `exec_runtime` first | Responsiveness-sensitive, foreground work   |
| `performance` | 10 ms      | FIFO                          | Throughput-heavy, fewer context switches    |
| `background`  | 20 ms      | FIFO                          | Low-priority work where latency doesn't matter |

Select with `--profile <name>`, e.g. `cerynth-scx --profile interactive`.

## Status file

Written atomically (temp file + `fsync` + `rename`, so readers never see a
partial write) to `/run/cerynth/scheduler.json` on start and on clean exit:

```json
{
  "running": true,
  "pid": 12345,
  "profile": "Balanced",
  "healthy": true,
  "started_at_ms": 1755000000000,
  "sched_ext_state": "enabled"
}
```

`sched_ext_state` is read live from `/sys/kernel/sched_ext/state` (same
source `cerynth-telemetry` uses), so it always reflects the kernel's actual
current state, not a cached guess.

## Safety

- No `unwrap()`s on the dequeue/dispatch path — errors are logged
  (`error.rs`) and the loop continues instead of panicking.
- `MAX_BATCH` (64) caps how many tasks a single dispatch pass drains before
  yielding, bounding worst-case latency and ensuring `Ctrl+C`/`SIGTERM` are
  noticed promptly instead of only between unbounded batches.
- `Ctrl+C`/`SIGTERM` both trigger a clean shutdown via the `ctrlc` crate,
  handing control back to the kernel's default scheduler.

## Testing

```
cargo test -p cerynth-scx       # unit tests for policy::order_tasks
cargo build --release -p cerynth-scx   # release binary for real-world runs
```

All four profiles have been stress-tested under full-core CPU load
(`yes > /dev/null` per core) with verified clean shutdown and correct
status-file transitions.
