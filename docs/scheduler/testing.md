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
profile has had every role. 100 iterations means 500 scheduler loads. With
the default 1.5 s hold, expect about 20–25 minutes.

After every start it checks:

- `/sys/kernel/sched_ext/state` is `enabled`;
- `/sys/kernel/sched_ext/enable_seq` went up, so a new scheduler really
  loaded rather than an old one still hanging around;
- `scheduler.json` and `scheduler-stats.json` name the right profile and
  have a fresh heartbeat;
- after the hold, `tasks_dispatched > 0`, the scheduler is still healthy,
  and a trivial `fork+exec` finishes in under 2 s.

After every stop it checks that the exit code is 0, sched_ext is `disabled`,
and both files say `running: false`. After SIGKILL it checks that the kernel
fell back on its own.

At the end it looks for stalls, `scx_bpf_error`, lockups and oopses in the
kernel log. One busy loop per CPU runs the whole time (`--load N` changes
that, `--load 0` turns it off).

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
| `result.json` | totals, `"result": "pass"` / `"fail"`, `unrecoverable` |

## 3. Fault tests

```bash
sudo ./scripts/scx/fault-test.sh                 # all scenarios, ~2 minutes
sudo ./scripts/scx/fault-test.sh --list
sudo ./scripts/scx/fault-test.sh --only heartbeat-freeze,dispatch-stall
```

| Scenario | Fault | Must happen |
| --- | --- | --- |
| `crash-sigkill` | SIGKILL | kernel falls back, machine responsive |
| `crash-sigsegv` | SIGSEGV | same |
| `status-deleted` | both JSON files deleted | recreated within 3 s, scheduler unaffected |
| `status-corrupt` | both files overwritten with invalid JSON | valid again within 3 s |
| `heartbeat-freeze` | SIGSTOP | heartbeat stops; kernel watchdog ejects the scheduler (logs `runnable task stall`); after SIGCONT it exits non-zero and writes `running: false` |
| `dispatch-errors` | 1 in 50 dispatches fails | stays healthy, `dispatch_errors > 0`, nothing lost, summary in log |
| `dispatch-stall` | every dispatch fails | scheduler exits by itself within ~2 s, before the kernel watchdog |
| `sysrq-disable` | SysRq-S | scheduler notices, exits non-zero, writes `running: false` |

After each scenario a fresh scheduler has to start and dispatch tasks. A
fault that leaves the machine unable to run the scheduler again is a
failure even if everything else passed.

During `heartbeat-freeze` and `dispatch-stall` the test script itself
pauses for a few seconds. Its own processes are waiting on the broken
scheduler too, and that is exactly the situation the watchdog is there for.

The dispatch faults use a test-only switch in the scheduler:

```bash
CERYNTH_SCX_INJECT_DISPATCH_ERRORS=50 sudo -E ./target/release/cerynth-scx
```

Every 50th dispatch attempt is then treated as failed before it reaches the
BPF side. The scheduler prints a `FAULT INJECTION ON` warning at startup
when this is set. It is never set by `cerynthd` or the systemd units.

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
