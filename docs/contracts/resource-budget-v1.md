# Resource Budget Contract v1

The typed interface between the resource mechanism (`crates/cerynth-resources`,
owned by Person 1), the admission decision (Person 3), and the Resource Studio
(Person 4). It is deliberately narrow: a request, an applied-status response,
and a usage snapshot. Everything is keyed by the **session UUID** from Person 3,
never by a PID.

`docs/contracts/runtime-v1.md` stays frozen. This is a new contract layered on
top of it; it does not rename or move any runtime path.

**Status:** Sprint 1 prototype (week 1, Nov 2026). `schema_version` is `1`.

## Why a separate contract

The runtime contract describes processes and files. This contract describes a
*boundary*: what a session is allowed to consume, and what the kernel actually
agreed to. Those are different questions:

- A controller may be compiled into the kernel but not delegated to the
  cgroup we can write.
- A limit may be readable but read-only, because systemd owns that cgroup.
- The kernel may round a byte value to a page boundary.

The contract therefore records **enforcement**, not just the requested number.
`enforced` and `observed` are not synonyms.

## Request: `ResourceBudget`

```json
{
  "schema_version": 1,
  "session_id": "8f14e45f-ea2b-4b5a-9d6e-3f0c2b7a1d44",
  "priority_class": "interactive",
  "limits": {
    "cpu_weight": 200,
    "cpu_max": { "quota_us": 50000, "period_us": 100000 },
    "memory_high_bytes": 1073741824,
    "memory_max_bytes": 2147483648,
    "io_weight": 200,
    "pids_max": 256
  }
}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `schema_version` | integer | Must be `1`. |
| `session_id` | UUID | Stable session identity from Person 3. |
| `priority_class` | enum | `interactive`, `balanced`, `background`, `inference`, `batch_compute`, `system_critical`. |
| `limits` | object | Requested controllers. Omitted fields are left alone. |

`priority_class` is a label, not a scheduler command. It is recorded so later
weeks can feed agent phases into scheduling experiments without changing this
schema.

### Limit fields

| Field | cgroup file | Range / format |
| --- | --- | --- |
| `cpu_weight` | `cpu.weight` | `1..=10000` |
| `cpu_max` | `cpu.max` | `{ quota_us > 0, period_us in 1000..=1000000 }` |
| `memory_high_bytes` | `memory.high` | bytes; must be `<= memory_max_bytes` when both are set |
| `memory_max_bytes` | `memory.max` | bytes |
| `io_weight` | `io.weight` | `1..=10000` |
| `pids_max` | `pids.max` | `>= 1` |

**An omitted field means "do not write this file", not "unlimited".** Any value
inherited from the parent cgroup stays in force. A caller that wants
unlimited must say so explicitly in a future schema revision; v1 has no
`unlimited` sentinel.

Requests are strict: unknown fields are rejected. A typo must fail loudly
rather than silently produce an unconstrained session.

## Response: `BudgetStatus`

The daemon returns the cgroup path plus one outcome per requested limit.

```json
{
  "schema_version": 1,
  "session_id": "8f14e45f-ea2b-4b5a-9d6e-3f0c2b7a1d44",
  "cgroup_path": "/sys/fs/cgroup/cerynth.slice/cerynth-session-8f14e45f.scope",
  "limits": {
    "cpu_weight": {
      "requested": 200,
      "applied": 200,
      "enforcement": "enforced"
    },
    "cpu_max": {
      "requested": { "quota_us": 50000, "period_us": 100000 },
      "applied": { "quota_us": 50000, "period_us": 100000 },
      "enforcement": "enforced"
    },
    "memory_max_bytes": {
      "requested": 2147483648,
      "enforcement": "observed",
      "note": "write memory.max failed: Permission denied"
    },
    "io_weight": {
      "requested": 200,
      "enforcement": "unsupported",
      "note": "write io.weight failed: No such file or directory"
    },
    "pids_max": {
      "requested": 256,
      "applied": 256,
      "enforcement": "enforced"
    }
  }
}
```

`applied` is omitted when the kernel value could not be read back or is
unlimited (`max`). `note` is omitted for a clean `enforced` result.

### Enforcement values

| Value | Meaning | Caller must assume |
| --- | --- | --- |
| `enforced` | The file was written and read back equal to the request. | The limit is active. |
| `observed` | The value is visible/measurable, but this host could not enforce it (read-only delegation, systemd-owned cgroup, kernel rounding, or the file reads back as `max`). | **The limit is not guaranteed.** |
| `unsupported` | The controller or file does not exist here (`ENOENT`). | The limit is absent. |

Admission (Person 3) must treat `observed` as "cannot guarantee" rather than
"granted". Dashboards (Person 4) must render the three states differently; a
green "limited" badge on an `observed` limit is a bug.

## Usage: `ResourceUsage`

Read from the session cgroup and its descendants.

```json
{
  "schema_version": 1,
  "session_id": "8f14e45f-ea2b-4b5a-9d6e-3f0c2b7a1d44",
  "cgroup_path": "/sys/fs/cgroup/cerynth.slice/cerynth-session-8f14e45f.scope",
  "populated": true,
  "pids": [4121, 4122],
  "cpu": {
    "usage_usec": 123456,
    "user_usec": 90000,
    "system_usec": 33456,
    "nr_periods": 10,
    "nr_throttled": 2,
    "throttled_usec": 4000
  },
  "memory": {
    "current_bytes": 1048576,
    "peak_bytes": 2097152,
    "high_bytes": 1073741824,
    "max_bytes": 2147483648,
    "events": {
      "low": 0,
      "high": 0,
      "max": 0,
      "oom": 0,
      "oom_kill": 0,
      "oom_group_kill": 0
    }
  },
  "pids_usage": { "current": 2, "max": 256, "max_events": 0 },
  "io": [
    {
      "device": "8:0",
      "rbytes": 4096,
      "wbytes": 8192,
      "rios": 3,
      "wios": 4,
      "dbytes": 0,
      "dios": 0
    }
  ],
  "pressure": {
    "cpu": {
      "some": { "avg10": 1.5, "avg60": 0.25, "avg300": 0.01, "total_us": 1234 },
      "full": { "avg10": 0.75, "avg60": 0.1, "avg300": 0.0, "total_us": 567 }
    }
  }
}
```

Notes:

- `pids` is the union of `cgroup.procs` for the session cgroup and every
  descendant cgroup, sorted and de-duplicated. It is the accounting answer to
  "did anything escape the boundary?".
- `memory.peak`, `memory.high`, `memory.max`, `pids.max`, `io`, and each
  pressure line are omitted when the file is absent on this kernel.
- Pressure values are copied from `cpu.pressure`, `memory.pressure` and
  `io.pressure` without interpretation. `avg*` are percentages, `total_us` is
  microseconds of stall.
- Counters are cumulative for the life of the cgroup, not deltas.

## Support matrix

`cerynth-resource-support` probes the host and prints the same information in
human-readable or JSON form. It is diagnostic, not part of the wire contract.

```bash
just resource-support
cargo run -p cerynth-resources --bin cerynth-resource-support -- --json
```

The matrix records, per limit: whether the file is present, whether it is
writable, and whether it is enforceable for child session cgroups. It also
reports PSI availability, `cgroup.kill`, `memory.peak`, and whether a systemd
user manager is available. The probe creates and removes a temporary child
cgroup when the root is writable, because systemd often makes a parent's own
limit files read-only while its children are writable.

## Ownership and consumers

| Consumer | Uses |
| --- | --- |
| Person 3 (admission) | Builds a `ResourceBudget`; must not grant more than the host can enforce. |
| Person 4 (Resource Studio) | Renders `BudgetStatus` and `ResourceUsage`; must distinguish enforcement states. |
| Person 2 (sandboxing) | Combines the session cgroup with its sandbox boundary. |
| Person 1 (mechanism) | Applies limits, reads usage, kills and removes the tree. |

## Change rules

- Renaming, removing, or changing the meaning of a field bumps
  `schema_version`.
- Adding an optional field does not bump it. Readers must ignore unknown
  fields in responses and usage snapshots.
- Requests reject unknown fields (`deny_unknown_fields`). This is intentional:
  a misspelled limit must not silently disappear.
- `Enforcement` is closed. A new state is a schema change, because consumers
  switch on it.

## Non-goals for v1

- No scheduler changes. cgroups first; `cerynth-scx` only if measurements show
  cgroups are insufficient.
- No hierarchical parent/child budgets yet (phase 6).
- No remote resources (phase 6).
- No admission policy. This crate validates a request and reports what the
  kernel did; Person 3 decides who gets what.
- No long-term storage. Usage is point-in-time; the telemetry crate owns
  history.

## Verifying the contract

```bash
just test-resource      # unit + cgroup integration tests
just resource-support   # which limits this host can actually enforce
```

The privileged integration cases skip with a reason when no writable cgroup
subtree exists. Set `CERYNTH_TEST_CGROUP_ROOT` to point at a delegated subtree
explicitly.
