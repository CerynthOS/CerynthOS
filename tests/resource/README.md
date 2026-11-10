# Resource-control tests

Exercises the cgroup v2 boundary described in
[`docs/contracts/resource-budget-v1.md`](../../docs/contracts/resource-budget-v1.md).

## What is covered

- cgroup v2 detection and controller discovery.
- Session subtree creation and removal, keyed by session UUID.
- Limit application, with `enforced` / `observed` / `unsupported` classified
  per limit instead of assumed.
- Usage accounting: `cpu.stat`, `memory.current`/`peak`/`events`,
  `pids.current`/`events`, `io.stat`, and PSI.
- Process-tree accounting across descendant cgroups.
- `cgroup.kill` cleanup with no surviving PIDs.

## Running

```bash
./tests/resource/run.sh
```

or, without the support matrix:

```bash
just test-resource
```

The privileged cases skip with an explicit `SKIP` line when no writable
cgroup v2 subtree is available. They do not fail the suite.

To point the tests at a specific delegated subtree:

```bash
CERYNTH_TEST_CGROUP_ROOT=/sys/fs/cgroup/user.slice/... ./tests/resource/run.sh
```

As root, the tests use the cgroup v2 mount directly. As an unprivileged user on
systemd, they use the user manager's delegated subtree and a transient
`systemd-run --user --scope`.

## Support matrix

```bash
just resource-support
```

This prints which limits this host can actually enforce, and why the others
cannot. `--json` emits the machine-readable form.
