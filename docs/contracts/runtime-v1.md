# CerynthOS Runtime Contract v1

The integration surface shared by the daemon, CLI, scheduler and policy
engine. Everything here is frozen for week 1. Changing a path or a schema in
this document requires agreement from all four tracks, because each one is
built against these values independently.

## Filesystem layout

| Path | Owner | Purpose |
| --- | --- | --- |
| `/usr/lib/cerynth/cerynthd` | Person 1 | Runtime daemon |
| `/usr/lib/cerynth/cerynth-scx` | Person 1 | sched_ext scheduler |
| `/usr/lib/cerynth/cerynth-boot-marker` | Person 1 | Boot marker writer |
| `/usr/bin/cerynthctl` | Person 1 | Control CLI |
| `/etc/cerynth/cerynth.toml` | Person 1 | Configuration |
| `/var/lib/cerynth/state.json` | Person 3 | Persistent runtime state |
| `/run/cerynth/cerynthd.sock` | Person 3 | Daemon IPC socket |
| `/run/cerynth/scheduler.json` | Person 2 | Scheduler status |
| `/run/cerynth/policy.json` | Person 4 | Shadow-mode recommendations |
| `/run/cerynth/boot-ok` | Person 1 | Boot completion marker |

`/run/cerynth` is created by systemd via `RuntimeDirectory=cerynth` in
`cerynthd.service`, and by the daemon itself when started by hand.

> `RuntimeDirectoryPreserve=yes` is **required** in `cerynthd.service`.
> Without it systemd deletes the whole of `/run/cerynth` whenever the daemon
> stops, destroying the boot marker and the scheduler status file.

## Path overrides

Every runtime path can be overridden so components can run unprivileged in
tests. Production defaults are the table above.

| Variable | Overrides |
| --- | --- |
| `CERYNTH_SOCKET` | `/run/cerynth/cerynthd.sock` |
| `CERYNTH_CONFIG` | `/etc/cerynth/cerynth.toml` |
| `CERYNTH_STATE` | `/var/lib/cerynth/state.json` |

Defined in `crates/cerynth-ipc/src/protocol.rs` and
`services/cerynthd/src/paths.rs`. Do not hardcode these paths anywhere else:
the daemon previously loaded state from one hardcoded path and saved it to a
different one, and the CLI kept its own copy of the socket path.

## Profiles

```
balanced
interactive
performance
background
```

Serialized lowercase. Defined once in `crates/cerynth-ipc/src/profile.rs`.

## Scheduler backends

```
mock    in-process stand-in used by tests
scx     drives a real cerynth-scx child process
```

Serialized lowercase. Defined in `crates/cerynth-ipc/src/backend.rs`.

## Configuration file

`/etc/cerynth/cerynth.toml`, shipped from `packaging/config/cerynth.toml`:

```toml
default_profile = "balanced"
adaptation_enabled = false
scheduler_backend = "scx"
scheduler_binary = "/usr/lib/cerynth/cerynth-scx"
```

`Config::load` falls back to built-in defaults when the file cannot be
parsed, and prints a warning to stderr when it does. A field added here
without a matching field on `Config` is ignored; a *value* that does not
deserialize discards the whole file. `shipped_default_config_parses` in
`crates/cerynth-config` guards against that.

## Scheduler status schema

`/run/cerynth/scheduler.json`, written atomically (write temp, fsync,
rename):

```json
{
  "running": true,
  "pid": 1234,
  "profile": "interactive",
  "sched_ext_state": "enabled",
  "healthy": true,
  "started_at_ms": 1785990000000
}
```

## Policy recommendation schema

`/run/cerynth/policy.json`:

```json
{
  "recommended_profile": "interactive",
  "confidence": 0.84,
  "reason": "High runnable load with frequent short-lived processes",
  "apply": false
}
```

`apply` stays `false` for the whole of week 1: recommendations are shadow
mode only and never drive the scheduler.

## Boot marker

`/run/cerynth/boot-ok`, written once per boot by
`cerynth-boot-marker.service`:

```
kernel=7.1.0
timestamp=2026-08-13T05:52:10Z
boot_id=b13e1be2-945a-4438-ab7b-16eaa56d319c
```

The logic lives in `packaging/scripts/cerynth-boot-marker`, not inline in
the unit file. systemd interprets `%` in `ExecStart` as a specifier, so a
`printf` format string written inline corrupts itself silently — `%s`
expands to the user's shell rather than staying a format placeholder.

## Socket permissions

`/run/cerynth/cerynthd.sock` is created mode `0660`, owned by `root:root`.
The daemon sets this explicitly rather than inheriting the umask.

Connecting to a Unix socket requires **write** permission, so `cerynthctl`
currently requires root:

```bash
sudo cerynthctl status
```

*Open question for Person 3:* whether to add a `cerynth` system group and
chown the socket to it, allowing unprivileged read-only commands the way
`docker.sock` works. That is a security-posture decision, not a packaging
one, so it is deliberately left open here rather than settled unilaterally.

## Verifying the contract

```bash
just vm-provision   # build + install into the running VM
just vm-smoke       # assert every item above actually exists
```

`scripts/vm/smoke-test.sh` is the executable form of this document.
