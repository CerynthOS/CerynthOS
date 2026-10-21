# cerynth-scx scheduler

How CerynthOS's `sched_ext` scheduler works and how its four profiles
differ. Code: `schedulers/cerynth-scx/`. Related pages:

- [stats.md](stats.md): the `/run/cerynth/scheduler-stats.json` interface
- [testing.md](testing.md): lifecycle stress and fault tests

## Two halves

```
 kernel                                   user space (cerynth-scx process)
 ───────────────────────────────          ──────────────────────────────────
 task becomes runnable
   │
   ├─ idle CPU free? ── yes ──► BPF half places it there directly
   │                            (counted as bpf.kernel_dispatches)
   no
   ▼
 "queued" ring buffer ─────────────────►  dispatch loop:
                                            1. take up to max_batch tasks
                                               (retries from last pass first)
                                            2. policy::order_tasks  (order)
                                            3. policy::slice_for    (slice)
 "dispatched" ring buffer ◄──────────────   4. dispatch each task
   │                                        5. notify_complete(pending)
   ▼                                        every 1 s: scheduler.json +
 BPF half puts the task in a CPU queue         scheduler-stats.json
   │
   ▼
 task runs for its slice
```

`main.bpf.c` and `src/bpf.rs` come from `scx_rustland_core` and are generic.
Everything CerynthOS-specific is in `src/policy.rs` and `src/profile.rs`.
Both are pure functions (tasks in, decisions out), which is why they can be
unit tested without root or a kernel.

## Profiles

| | balanced | interactive | performance | background |
| --- | --- | --- | --- | --- |
| **Order in batch** | FIFO | starved first, then shortest `exec_runtime` first | FIFO | starved first, then short bursts, then CPU hogs (arrival order within each lane) |
| **Slice** | 5 ms × weight/100, clamped 1–20 ms | 2 ms ÷ tasks-per-CPU, min 0.5 ms | fixed 10 ms | short burst 5 ms, hog 20 ms |
| **Max batch** | 64 | 32 | 128 | 128 |
| **Starvation rescue** | none needed (never reorders) | waited ≥ 20 ms | none needed | waited ≥ 100 ms |
| **Goal** | fair default | latency | throughput, few context switches | low overhead without hurting foreground work |

Terms used in the table:

- **`exec_runtime`**: CPU time a task has used since it last slept. Small
  means "just woke up / does short bursts" (a keypress handler). Large
  means "has been crunching for a while" (a compiler).
- **Short burst**: `exec_runtime < 2 ms`, meaning it hasn't used even one
  interactive slice yet.
- **Weight**: the kernel's version of the nice value. 100 is nice 0, and
  higher means more important. Only `balanced` scales by it.
- **Starved**: time since the task last left a CPU is above the profile's
  threshold. Tasks that have never run (`stop_ts == 0`) are never counted
  as starved. They already sort early because their `exec_runtime` is 0.

Why each profile is built the way it is:

- **balanced**: the baseline. Order is arrival order, so nothing can be
  overtaken. The slice follows weight, so a nice -5 task gets proportionally
  more CPU per turn than a nice +5 one.
- **interactive**: short slices that shrink further as the queue grows. With
  8 tasks waiting per CPU, each gets about a quarter of the time it would get
  on an idle machine, so everyone gets a turn sooner. Short-running tasks go
  to the front. The starvation rescue stops a steady stream of short tasks
  from keeping a long-running task at the back indefinitely.
- **performance**: long, fixed slices and big batches. Fewer trips between
  kernel and user space, fewer context switches, and behaviour that doesn't
  change with load, which suits benchmarks and builds.
- **background**: the longest slice for CPU hogs, but a waking short-burst
  task goes ahead of them and gets a normal slice. Bulk work runs cheaply
  without making the occasional interactive task wait 20 ms for a turn.

### Every dispatched task gets an explicit slice

Before week 3, tasks were dispatched with `slice_ns = 0`, which the code
comment described as "use the default". The kernel does not do that
(`kernel/sched/ext.c`, `scx_dsq_insert`):

```c
if (slice)
        p->scx.slice = slice;
else
        p->scx.slice = p->scx.slice ?: 1;
```

A slice of 0 means "keep whatever is left", and if nothing is left the task
gets **1 ns**. A CPU-bound task that had used up its slice was therefore
preempted at the next timer tick, whatever the profile. The profile slice
only reached tasks the BPF half dispatched by itself. `policy::slice_for`
now always returns a non-zero slice, and a unit test
(`every_slice_is_non_zero`) checks it.

## Safety

The kernel's default scheduler is the safety net. All of these end with the
BPF scheduler unloaded and Linux scheduling the machine again:

| What goes wrong | What happens | Tested by |
| --- | --- | --- |
| Clean stop (SIGTERM / SIGINT) | scheduler detaches, writes `running: false` to both files, exits 0 | `stress-lifecycle.sh` |
| Crash / SIGKILL | the kernel unloads the BPF scheduler as soon as the process is gone, in well under a second (see "Dying quickly" below) | `fault-test.sh crash-*`, `stress-lifecycle.sh` |
| Loading fails before the scheduler ever ran | scheduler detaches, writes `running: false`, exits with code 75, "start me again" (see "Starting again" below) | `fault-test.sh load-failure` |
| Kernel asks for a restart (a CPU went on/offline) | same: exit code 75 | unit test of the decision only |
| Scheduler frozen (SIGSTOP, deadlock) | kernel watchdog: a runnable task waiting > 5 s (`timeout_ms`) ejects the scheduler | `fault-test.sh heartbeat-freeze` |
| A dispatch fails | task kept in `pending` and retried next pass, never dropped | `fault-test.sh dispatch-errors` |
| Dispatches keep failing | no successful dispatch for 2 s → scheduler exits by itself, before the 5 s watchdog | `fault-test.sh dispatch-stall` |
| sched_ext disabled underneath (SysRq-S) | scheduler notices, writes `running: false`, exits non-zero | `fault-test.sh sysrq-disable` |
| Status files deleted / corrupted | rewritten on the next 1 s tick | `fault-test.sh status-*` |

Why a failed dispatch must be retried: once the Rust loop has taken a task
out of the queue, the kernel considers it the scheduler's job. If the loop
logs the error and moves on, that task never runs again, and five seconds
later the watchdog ejects the whole scheduler over one lost task.

### Dying quickly

The kernel unloads the BPF half only once the scheduler *process* is
completely gone, and while the process is dying nobody is dispatching
tasks. So anything that slows the process's death freezes the machine for
that long. Two things did (`src/process.rs`):

- **The helper thread.** The process has a second thread (started by the
  `ctrlc` crate to wait for signals). The dispatch loop's own thread is
  placed on a CPU by the BPF half directly, but every other task on the
  machine, including that helper thread, needs the dispatch loop to give it
  a CPU. On SIGKILL the helper must run once more to exit. With every CPU
  busy it waited for a dispatch loop that was already dead, the process
  could not finish dying, and the BPF half stayed loaded until the kernel
  watchdog ejected it. Measured with `stress-lifecycle.sh` on a 24-CPU
  machine under full load: **84 of 99 SIGKILLs took 5–9 s** to fall back.
  The helper thread is now moved to the kernel's real-time class at
  startup, which the kernel schedules itself, so it can always exit. Same
  test, same machine, after the fix: **100 of 100 SIGKILLs fell back in at
  most 193 ms** (average 142 ms), with no watchdog ejection in the kernel
  log.
- **Core dumps.** A core dump is written by a helper program the kernel
  starts while the crashed process is still half-alive, and that helper
  would need CPU time from the scheduler that just crashed. Core dumps are
  switched off for this process (`CERYNTH_SCX_ALLOW_COREDUMP=1` turns them
  back on for debugging).

`kill_hard` in `scripts/scx/test-lib.sh` times every hard kill and fails if
the fallback takes more than 3 s, so a regression here shows up as a number.

### Starting again

One scheduler process can load the BPF scheduler exactly once. While
loading, the vendored `scx_rustland_core` code installs a seccomp filter
that makes every later `mmap()` call in the process fail. That is on
purpose: the dispatch loop must never take a page fault, because resolving
one can need a kernel thread that is itself waiting for the dispatch loop.

The filter cannot be removed, and anything the process executes inherits
it. Loading BPF needs `mmap()`, and so does starting any program at all. So
neither a second load inside the same process nor re-executing the binary
can work. Re-executing was tried first, and `fault-test.sh` caught it: the
new image died in the dynamic loader with
`cannot create search path array: Cannot allocate memory`.

So when a start did not work out but a fresh one is expected to, the
scheduler detaches, writes `running: false` and says so with its exit code:

| Exit code | Meaning | What the supervisor should do |
| --- | --- | --- |
| 0 | clean stop (SIGTERM / SIGINT) | nothing |
| 75 (`EX_TEMPFAIL`) | the load failed before the scheduler ever ran, or the kernel asked for a restart | start it again, a limited number of times |
| any other | a real error: ejected by the watchdog, SysRq-S, stall bail-out, another scheduler already loaded | do not restart automatically |

Only a failure *before the first dispatch pass* counts as a failed load. A
scheduler that ran and was then ejected is never asked to be started again,
because that would fight whatever stopped it.

As of week 3 `cerynthd` does not look at the exit code. A start that ends
in 75 is reported like any other failed start (`scheduler exited before
becoming healthy`). The test scripts do act on it: `start_healthy` in
`scripts/scx/test-lib.sh` launches up to 3 times and counts every repeat as
`load_retries` in `result.json`.

Errors on the hot path are counted, not printed. Once per second the
heartbeat prints one summary line if anything new failed. A burst of
thousands of errors can't flood the journal or block the loop on a full
stderr pipe.

## Known issues

- On kernel 7.2 every load prints
  `sched_ext: Writing directly to p->scx.slice/dsq_vtime is deprecated`
  three times. It comes from the vendored `main.bpf.c`
  (`scx_rustland_core 2.4.11`), not from our code, and is harmless today.
  A future kernel may turn it into a load failure. The fix is a newer
  pinned `scx`, which is a platform decision.
- `ops.init_task() failed (-12)` while loading, about once in 600 loads
  under full CPU load. The kernel sets up per-task storage for every task
  when a scheduler is enabled, and one small allocation occasionally fails.
  Nothing is loaded when it happens. The scheduler exits with code 75, and
  the one time this was seen the next start worked.

## Running it

```bash
cargo build --release -p cerynth-scx
sudo ./target/release/cerynth-scx --profile interactive

# another terminal
watch -n1 jq . /run/cerynth/scheduler-stats.json
cat /sys/kernel/sched_ext/state        # enabled
```

Ctrl+C or SIGTERM stops it cleanly. Through the daemon:
`sudo cerynthctl profile set <profile>`. A profile change restarts the
scheduler with the new `--profile`.
