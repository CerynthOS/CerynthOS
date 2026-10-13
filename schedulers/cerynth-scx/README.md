# cerynth-scx

CerynthOS's sched_ext scheduler. Built on `scx_rustland_core`. The BPF
plumbing (`main.bpf.c`, `bpf.rs`, `bpf_intf.rs`, `bpf_skel.rs`) is shared,
generic infrastructure. The Rust side in `src/` is the actual scheduling
*policy*.

Design, profiles and safety model: [`docs/scheduler/README.md`](../../docs/scheduler/README.md).

## Module layout

- `main.rs`: entry point. CLI parsing, the dispatch loop (batching, retry
  of failed dispatches, stall bail-out), heartbeat, status-file lifecycle.
- `profile.rs`: the four profiles and their tuning knobs (`Params`).
- `policy.rs`: pure, BPF-free decisions. `order_tasks` (dispatch order and
  starvation rescue) and `slice_for` (per-task time slice). Unit tested
  with no kernel or root.
- `stats.rs`: runtime counters and the `/run/cerynth/scheduler-stats.json`
  snapshot.
- `status.rs`: `/run/cerynth/scheduler.json` and the atomic JSON writer
  both files share.
- `error.rs`: typed errors for dequeue/dispatch failures.

## Profiles

| Profile       | Slice                              | Order in batch                                  | Max batch |
|---------------|------------------------------------|-------------------------------------------------|----------:|
| `balanced`    | 5 ms × weight, clamped 1–20 ms     | FIFO                                            | 64        |
| `interactive` | 2 ms ÷ tasks per CPU, min 0.5 ms   | starved first, then shortest `exec_runtime`     | 32        |
| `performance` | fixed 10 ms                        | FIFO                                            | 128       |
| `background`  | 5 ms short bursts, 20 ms CPU hogs  | starved first, then short bursts, then hogs     | 128       |

Select with `--profile <name>`, e.g. `cerynth-scx --profile interactive`.

## Output files

Both are written atomically (temp file, `fsync`, `rename`, so readers never
see a partial write), at startup, every second, and on clean exit.
Deleted or corrupted files are rewritten on the next tick.

`/run/cerynth/scheduler.json`, the status file defined in
`docs/contracts/runtime-v1.md`:

```json
{
  "running": true,
  "pid": 12345,
  "profile": "balanced",
  "healthy": true,
  "started_at_ms": 1755000000000,
  "sched_ext_state": "enabled",
  "heartbeat": 1755000042
}
```

`healthy` is true only while `/sys/kernel/sched_ext/state` reads `enabled`.
A live process whose BPF scheduler the kernel already ejected reports
`running: true, healthy: false`. `heartbeat` is whole Unix seconds, and
cerynthd treats the scheduler as dead once it is more than 10 s old.

`/run/cerynth/scheduler-stats.json` holds runtime counters (dispatches,
batch sizes, starvation rescues, errors, plus the BPF side's own counters).
Schema: [`docs/scheduler/stats.md`](../../docs/scheduler/stats.md).

## Safety

- Every dispatched task gets an explicit, non-zero slice. A slice of 0
  would make the kernel keep the task's leftover slice, or give it 1 ns
  once that is used up.
- A failed dispatch is retried on the next pass, never dropped. A dropped
  task would never run again, and the kernel watchdog would eject the
  scheduler 5 s later.
- If no dispatch succeeds for 2 s, the scheduler exits by itself and the
  kernel falls back to its default scheduler.
- No `unwrap()`s on the dequeue/dispatch path. Errors are counted and
  summarized once per heartbeat instead of logged one by one.
- `Ctrl+C`/`SIGTERM` trigger a clean shutdown through the `ctrlc` crate,
  handing control back to the kernel's default scheduler.

## Testing

```
cargo test -p cerynth-scx                               # unit tests, no root
cargo build --release -p cerynth-scx
sudo ./scripts/scx/stress-lifecycle.sh --iterations 100 # start/stop/kill cycles
sudo ./scripts/scx/fault-test.sh                        # crashes, freezes, bad files
```

Details: [`docs/scheduler/testing.md`](../../docs/scheduler/testing.md).
