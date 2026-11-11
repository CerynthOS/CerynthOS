# AgentSession v1alpha1

Status: **experimental Person 3 implementation, pending cross-track review**.
This is separate from the scheduler's frozen `runtime-v1.md` interface. The
master vision's YAML document is not automatically a supported wire format.

The Rust types in `cerynth-agent-api` and generated
[JSON Schema](schemas/agent-api-v1alpha1.schema.json) define accepted JSON.
The generated [Session response schema](schemas/agent-session-v1alpha1.schema.json)
defines persisted and returned session records.
Unknown fields and unsupported versions are rejected. Regenerate the schema with:

```bash
cargo run --locked -p cerynth-agent-api --bin cerynth-agent-schema > docs/contracts/schemas/agent-api-v1alpha1.schema.json
cargo run --locked -p cerynth-agent-api --bin cerynth-agent-schema -- --session > docs/contracts/schemas/agent-session-v1alpha1.schema.json
```

## Identity and specification

Every session has a UUID, daemon-derived owner UID, canonical workspace,
timestamps, lifecycle, boundary, bounded events, and execution evidence. Its
identity survives process exit. Execution evidence includes PID/start tick/boot
ID, allocation UUID, scope unit, parent/task identities, operation ledger,
messages, artifacts, worktree preparation journal, normalized harness events,
Guard profile digest and pending process/authorization cleanup state.

```json
{
  "name": "coding-agent",
  "harness": "open_code",
  "workspace": "/absolute/existing/workspace",
  "model": {
    "name": "qwen2.5:3b", "mode": "local", "endpoint_id": "local",
    "cloud_consent": false, "disable_thinking": true
  },
  "runtime": {
    "project": "sample", "parent_id": null,
    "network": "host", "isolation": "required", "require_cgroup": true,
    "capabilities": [], "message_peers": [],
    "budget": {
      "cpu_weight": 100, "memory_high_bytes": 1073741824,
      "memory_max_bytes": 2147483648, "tasks_max": 128,
      "max_subagents": 4, "inference_concurrency": 2,
      "gpu_memory_bytes": 0, "timeout_seconds": 300
    }
  }
}
```

Runtime and budget fields have defaults. Default networking is `deny`; `host`
above is explicit and needed for current localhost inference. Names/models are
1–256 bytes without control characters. Workspaces must exist. The owner supplies
root grants. Children inherit a subset of capabilities, sharing peers,
network/isolation authority, individual limits and bounded total memory/GPU/
concurrency reservations. Current independent systemd scopes do not enforce
aggregate parent kernel limits.

Harness values: `inference_probe`, `generic_pty`, `open_code`, `hermes`, `codex`,
`claude_code`, `acp`. ACP is a negotiated transport library; selecting `acp` does
not yet provide a daemon launch recipe. An endpoint probe performs no tools and
uses only the daemon's local Ollama origin. Named endpoints use the coordinator
for supported native harnesses. Cloud mode requires explicit session consent
and an owner-configured consented remote endpoint.

## Lifecycle and evidence

| Current | Allowed forward transitions |
| --- | --- |
| created | preparing |
| preparing | starting |
| starting | running |
| running | waiting_for_inference, waiting_for_tool, waiting_for_approval, pausing, completing |
| any waiting state | running, pausing |
| pausing | paused |
| paused | resuming |
| resuming, recovering | starting |
| completing | completed |
| any nonterminal | failed, cancelled, recovering |
| completed, failed, cancelled | none |

Schema pause/resume states are not an adapter capability guarantee. Daemon
adapters currently offer cancellation, not native pause/resume. A retry uses a
new session. Unknown completion after a crash remains unknown; issued operations
never silently replay. Parent termination cancels descendants. Harness completion
is a process outcome; task correctness requires independent artifact/test checks.

Modes are `managed` (current Bubblewrap + systemd launch), `integrated` (owned
process with weaker opted-in controls), and `observed` (visibility only).
Boundary labels are `unenforced_prototype`, `process_group`, `bubblewrap`,
`bubblewrap_systemd`, and `guard_systemd` for the pinned Person 2 profile.
They describe actual controls, not a security audit.
Observed processes never acquire kill authority or a sandbox label.

`runtime.guard` explicitly requires Guard for an offline `generic_pty` launch.
Supported typed grants are `fs.read`, `fs.write`, `session.spawn`, `tool.exec`,
and `gpu.use`, with bounded target/mode/optional spawn limit. Inference/egress/
credential grants and native harness launches through Guard are rejected.
Registration/cleanup ambiguity remains recorded; there is no weaker fallback.
Worktree preparation must finish before creating a Guard session. This optional
field does not imply all generic or native processes use Guard.

Events contain schema version, stable event ID, trace/session/parent/task IDs,
timestamp, sequence, category, name and attributes. Runtime evidence is distinct
from lower-trust `harness_report` evidence. Oldest/last sequence fields support
gap detection after bounded retention. Tool errors remain errors. Raw prompts,
tool arguments and output are excluded from these events. Raw terminal output
is separately private/bounded and may contain sensitive harness output.

## Owner API

One newline-delimited JSON request and response per Unix connection:

```json
{"version":1,"operation":{"type":"status","id":"SESSION_UUID"}}
```

```json
{"version":1,"result":{"Ok":{"id":"...","state":"created"}}}
```

Status above is abbreviated. Errors use `{"version":1,"result":{"Err":"message"}}`.
CLI `request FILE` accepts the operation alone and wraps the envelope.

| Operations | Semantics |
| --- | --- |
| create, list, status, archive | Registry and terminal archival |
| start, harness_start, input, output, stop, cancel, cleanup | Owned process/PTY supervision, bounded IO and explicit cleanup retry |
| infer, models, adapters | Local probe and capability/version discovery |
| events, process_tree, diagnostics, inventory | Structured evidence and supported observations |
| delegate, graph, send_message, messages, authorize_sharing | Bounded owner-supervised orchestration |
| worktree, collect_artifact, artifacts | Git workspaces and immutable file evidence |
| set_phase, request_resources | Semantic reports; no self-authorized limit increase |
| gpu_submit, gpu_jobs, gpu_finish | Cooperative GPU reservations and queue state |
| observe | Same-owner visibility without process control |
| discover_harness | Configured OpenCode native API discovery; no import or process control |
| a2a | Experimental sponsored JSON-RPC task subset |

Precise fields are in the schema. Start binds an immutable launch fingerprint to
an operation UUID. Replays return prior evidence with `action_executed: false`;
changed details or use by another session are rejected. This does not deduplicate
arbitrary effects inside a harness.

Owner sockets authenticate same UID and private-directory access. The default
sandbox cannot see them. This is not a control API for arbitrary unisolated
same-user agents. Per-session random tokens authenticate the separate inference
data plane and cannot call administrative operations.

Limits: 64 KiB request frame, 32 KiB input, 4 MiB response, 128 active registry
records, 16 supervised processes, 32 concurrent socket clients, 1024 events and
256 normalized harness records per session, 1 MiB raw output per session, 256 MiB
artifact storage, 4096 archived operation IDs. State is private, bounded,
atomically replaced and fsynced. Corrupt/unsupported state fails closed.
The inference coordinator has its own 180-second total deadline, 256 KiB HTTP
body, 4 MiB HTTP output and 4096-token output bound.

## Cross-track review still required

Person 1 must review allocation/cgroup hierarchy, pressure signals and scheduler
weight semantics. Person 2 must review immutable authorization, credential
issuance, sandbox receipts and authoritative tool mediation. Person 4 must review
event consumption, gaps and evidence/enforcement labels. The
[architecture document](../architecture/person3-runtime.md) records proposed
handoffs. No cross-track agreement is claimed.
