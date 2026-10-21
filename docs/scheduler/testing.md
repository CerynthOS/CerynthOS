# Testing cerynth-scx

Three layers, from fastest to most realistic.

## 1. Unit tests (no root, no kernel)

```bash
cargo test -p cerynth-scx
```

These cover the ordering and slice rules for every profile, starvation
rescue, stats counting, the frozen field names of both JSON files, and the
atomic writer recovering from a deleted directory or a corrupted file.

## 2. Lifecycle stress

```bash
cargo build --release -p cerynth-scx
sudo ./scripts/scx/stress-lifecycle.sh --iterations 100
```

One iteration:

```
start A → SIGTERM → start B → change to C → change to D → SIGINT
        → restart A → SIGKILL → recover with B → SIGTERM
```

A–D rotate through the four profiles, so after four iterations every
profile has had every role. 100 iterations means 600 scheduler loads. With
the default 1.5 s hold, expect about 25–30 minutes.

After every start it checks:

- `/sys/kernel/sched_ext/state` is `enabled`;
- `/sys/kernel/sched_ext/enable_seq` went up, so a new scheduler really
  loaded rather than an old one still hanging around;
- `scheduler.json` and `scheduler-stats.json` name the right profile and
  have a fresh heartbeat;
- after the hold, `tasks_dispatched > 0`, the scheduler is still healthy,
  and a trivial `fork+exec` finishes in under 2 s.

A launch that exits with code 75 ("start me again", see "Starting again" in
[README.md](README.md)) is repeated, up to 3 launches per start, the way a
supervisor is expected to. Every repeat is counted as `load_retries` in
`result.json`. Any other early exit fails the start.

After every stop it checks that the exit code is 0, sched_ext is `disabled`,
and both files say `running: false`.

After the SIGKILL it times how long the kernel takes to fall back and fails
if that is more than 3 s. A healthy fallback takes a few hundred ms. 5 s or
more means the process could not finish dying and the kernel watchdog had
to eject the scheduler (see "Dying quickly" in [README.md](README.md)).

After every iteration it reads the kernel log for that iteration and fails
on any stall, `scx_bpf_error`, lockup or oops. One busy loop per CPU runs the
whole time (`--load N` changes that, `--load 0` turns it off).

If a step fails, the script gets back to a clean state (SIGTERM → SIGKILL
→ SysRq-S) and moves to the next iteration. If even SysRq-S can't unload
the scheduler, it stops and reports **unrecoverable**, which is the outcome
this test exists to catch. `progress.log` is synced after every iteration,
so if the machine ever hangs, the last line shows where.

Output: `artifacts/scx-lifecycle/<timestamp>/`

| File | Contents |
| --- | --- |
| `progress.log` | every check, PASS/FAIL |
| `scheduler.log` | the scheduler's own stdout/stderr, one section per launch |
| `kernel.log` | kernel messages during the run |
| `result.json` | totals, `"result": "pass"` / `"fail"`, `unrecoverable`, `fallback_ms_max` / `fallback_ms_avg` over all SIGKILLs, `load_retries` |

### How the kernel log is read

The scripts write a marker line into the kernel log (`/dev/kmsg`) at the
start of each step and read everything after it with `dmesg`. No timestamps
are compared. The first version compared `/proc/uptime` with the `[seconds]`
stamps in `dmesg`. Those are different clocks: uptime keeps counting while a
laptop is suspended and the kernel log's clock does not. On a machine that
had been asleep, every line looked older than the mark, the log came back
empty, and "kernel log clean" passed without having read anything.

Two guards keep that from happening again. The first start of every run
must find its own `BPF scheduler ... enabled` line in the log, and a step
whose marker can't be found fails instead of passing.

## 3. Fault tests

```bash
sudo ./scripts/scx/fault-test.sh                 # all scenarios, ~1.5 minutes
sudo ./scripts/scx/fault-test.sh --list
sudo ./scripts/scx/fault-test.sh --only heartbeat-freeze,dispatch-stall
```

| Scenario | Fault | Must happen |
| --- | --- | --- |
| `crash-sigkill` | SIGKILL | kernel falls back within 3 s without the watchdog, machine responsive |
| `crash-sigabrt` | SIGABRT, what a Rust abort raises | same |
| `load-failure` | the load is made to fail right after attaching | scheduler leaves by itself with exit code 75 and `running: false`; a start whose first launch fails comes up on the second launch |
| `status-deleted` | both JSON files deleted | recreated within 3 s, scheduler unaffected |
| `status-corrupt` | both files overwritten with invalid JSON | valid again within 3 s |
| `heartbeat-freeze` | SIGSTOP | heartbeat stops; kernel watchdog ejects the scheduler (logs `runnable task stall`); after SIGCONT it exits non-zero and writes `running: false` |
| `dispatch-errors` | 1 in 50 dispatches fails | stays healthy, `dispatch_errors > 0`, nothing lost, summary in log |
| `dispatch-stall` | every dispatch fails | scheduler exits by itself within ~2 s, before the kernel watchdog |
| `sysrq-disable` | SysRq-S | scheduler notices, exits non-zero, writes `running: false` |

After each scenario a fresh scheduler has to start and dispatch tasks. A
fault that leaves the machine unable to run the scheduler again is a
failure even if everything else passed.

There is no SIGSEGV scenario on purpose. Rust's runtime installs its own
SIGSEGV handler to report stack overflows. A SIGSEGV sent from outside has
no faulting instruction to return to, so that handler uninstalls itself and
the program simply carries on. A real segfault inside the scheduler still
kills it.

During `heartbeat-freeze` and `dispatch-stall` the test script itself
pauses for a few seconds. Its own processes are waiting on the broken
scheduler too, and that is exactly the situation the watchdog is there for.

Two scenarios use test-only switches in the scheduler:

```bash
CERYNTH_SCX_INJECT_DISPATCH_ERRORS=50 sudo -E ./target/release/cerynth-scx
CERYNTH_SCX_INJECT_LOAD_FAILURE=1 sudo -E ./target/release/cerynth-scx
```

With the first, every 50th dispatch attempt is treated as failed before it
reaches the BPF side. With the second, the start is treated as a failed
load. The scheduler prints a `FAULT INJECTION ON` warning at startup when
either is set. Neither is ever set by `cerynthd` or the systemd units.

Output: `artifacts/scx-faults/<timestamp>/`, same layout as above, with a
per-scenario pass/fail map in `result.json`.

## Before running either script

Both drive `cerynth-scx` directly and refuse to start if another sched_ext
scheduler is loaded. If `cerynthd` started one: `sudo cerynthctl stop`.
Both re-run themselves under `sudo` if needed, and need `jq` (already in the
VM image).

Run them in the VM first (`./scripts/vm/run-cerynth-vm.sh`). These tests are
designed to recover from everything they do, but SysRq-S and watchdog
ejections are better seen for the first time on a machine you can reset.
