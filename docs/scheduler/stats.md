# Scheduler stats interface

`/run/cerynth/scheduler-stats.json` — written by `cerynth-scx`, read by
telemetry, `cerynthctl`/observability and the adaptation controller.

**Frozen as of the week-3 sprint (Aug 17, 2026).** Renaming, removing or
changing the meaning of a field needs agreement from everyone who reads the
file, plus a `schema_version` bump. Adding a field does not: readers must
ignore fields they don't recognise. `snapshot_field_names_are_frozen` in
`schedulers/cerynth-scx/src/stats.rs` fails if the field set changes, as a
reminder to update this page.

## When it is written

- Once at startup, with zero counters.
- Every second while running (same tick as the `scheduler.json` heartbeat).
- Once at shutdown with `"running": false`, after the BPF scheduler has been
  detached. A scheduler that is SIGKILLed or crashes can't write this last
  one, so the file goes stale instead. Check `heartbeat` before trusting
  `running`.

Every write is atomic (temp file, fsync, rename), so a reader never sees a
half-written file. If the file or `/run/cerynth` is deleted, or something
else overwrites it, the next tick puts it back.

Counters start at zero each time the scheduler starts. A profile change is a
restart, so counters reset on every profile change.

## Example

```json
{
  "schema_version": 1,
  "running": true,
  "pid": 4121,
  "profile": "interactive",
  "started_at_ms": 1787000000000,
  "uptime_ms": 92013,
  "heartbeat": 1787000092,
  "tasks_dequeued": 184233,
  "tasks_dispatched": 184233,
  "tasks_pending": 0,
  "dispatch_errors": 0,
  "dequeue_errors": 0,
  "batches_processed": 5821,
  "avg_batch_size": 31.65,
  "max_batch_size": 32,
  "starvation_rescues": 44,
  "bpf": {
    "nr_online_cpus": 8,
    "nr_running": 6,
    "nr_queued": 0,
    "nr_scheduled": 0,
    "user_dispatches": 160112,
    "kernel_dispatches": 52410,
    "cancel_dispatches": 31,
    "bounce_dispatches": 0,
    "failed_dispatches": 0,
    "sched_congested": 0
  }
}
```

## Fields

The scheduler is split in two halves (see `docs/scheduler/README.md`): a
user-space Rust loop that picks the order and slice for each task, and a BPF
half inside the kernel that hands tasks over and puts them on CPUs. The
top-level counters come from the Rust loop. The `bpf` object is read out of
the BPF half's memory.

| Field | Type | Meaning |
| --- | --- | --- |
| `schema_version` | int | `1`. Bumped only on incompatible changes. |
| `running` | bool | `false` only in the final write on a clean exit. |
| `pid` | int | Scheduler process id. |
| `profile` | string | `balanced` / `interactive` / `performance` / `background` |
| `started_at_ms` | int | Unix ms when this scheduler instance loaded. |
| `uptime_ms` | int | ms since `started_at_ms` at the time of this write. |
| `heartbeat` | int | Unix **seconds** of this write. Same meaning as in `scheduler.json`. |
| `tasks_dequeued` | int | Tasks the Rust loop took from the BPF half. |
| `tasks_dispatched` | int | Tasks the Rust loop successfully handed back with a CPU and slice. |
| `tasks_pending` | int | Tasks waiting for a retry after a failed dispatch. Normally `0`. |
| `dispatch_errors` | int | Failed dispatch attempts. Each one is retried, not dropped. |
| `dequeue_errors` | int | Errors reading from the BPF ring buffer. |
| `batches_processed` | int | Dispatch passes that had at least one task. Idle passes don't count. |
| `avg_batch_size` | float | Mean tasks per batch, 2 decimals. |
| `max_batch_size` | int | Largest batch seen. Capped by the profile's `max_batch`. |
| `starvation_rescues` | int | Tasks moved to the front of their batch because they had waited too long. Always `0` for `balanced` and `performance`, which never reorder. |
| `bpf.nr_online_cpus` | int | CPUs the scheduler manages. |
| `bpf.nr_running` | int | Tasks on a CPU right now. |
| `bpf.nr_queued` | int | Tasks waiting in the ring buffer for the Rust loop. |
| `bpf.nr_scheduled` | int | Tasks the Rust loop holds and hasn't dispatched yet. |
| `bpf.user_dispatches` | int | Dispatches to the CPU the Rust loop picked. |
| `bpf.kernel_dispatches` | int | Dispatches the BPF half made by itself, e.g. a waking task placed straight on an idle CPU. |
| `bpf.cancel_dispatches` | int | Dispatches dropped because the task changed state in between. Small numbers are normal. |
| `bpf.bounce_dispatches` | int | Dispatches redirected because the chosen CPU wasn't allowed for that task. |
| `bpf.failed_dispatches` | int | Dispatches the BPF half could not complete. |
| `bpf.sched_congested` | int | Times the ring buffer to user space was full. |

## Reading it

```bash
jq . /run/cerynth/scheduler-stats.json

# live, once a second
watch -n1 "jq -c '{profile, uptime_ms, tasks_dispatched, avg_batch_size, starvation_rescues, dispatch_errors}' /run/cerynth/scheduler-stats.json"
```

Rates (dispatches/s and so on) are not stored. Read the file twice and
divide the difference by the difference in `uptime_ms`.

## Health, for consumers

Treat the scheduler as healthy only if all of these hold:

1. `heartbeat` is less than 10 s old (same threshold as cerynthd's
   `HEARTBEAT_TIMEOUT_SECS`);
2. `running` is `true`;
3. `/sys/kernel/sched_ext/state` is `enabled`.

`dispatch_errors` going up with `tasks_pending` back at `0` is the retry
path doing its job. `tasks_pending` staying above zero is a problem. If no
dispatch succeeds for 2 s the scheduler exits by itself, and the kernel
falls back to its default scheduler.
