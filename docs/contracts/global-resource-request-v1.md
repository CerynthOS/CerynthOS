# Global resource request candidate v1

This is the P3-087 design and executable planner, paired with Person 1's actual
[ResourceBudget v1](resource-budget-v1.md) at commit
`648c149be39f5ea8df612b3266857472976238eb`. Team agreement remains P3-010.
An opt-in shared managed-domain reservation authority is now implemented as
described below. Complete accounting of all other services is still required
before calling any observation machine-wide.

`cerynth_resources::global::Demand` has schema version 1, immutable request and
session UUIDs, optional parent UUID, project, requested budget, optional minimum,
GPU device identity and inference endpoint identity. A minimum must fit within
the request. A child request must fit the trusted parent's authority.

`OwnerSnapshot` comes from the trusted allocation owner, never from an agent. It
contains a timestamp, an explicit complete-accounting attestation, CPU/RAM
capacity and reservations, parent authority, per-device GPU capacity/free memory/
reservations/desktop reserve, inference capacity/reservations and a pressure
gate. Observations older than five seconds or more than one second in the
future are rejected. Unknown GPU memory or missing GPU admission support queues
work. The GPU guarantee is cooperative reservation only.

The pure `plan` function returns one proposal spanning CPU, RAM, GPU and
inference: accept, reduce within the owner-provided minimum, queue temporarily,
or reject an impossible/unauthorized demand. Incomplete accounting always
queues. Every proposal says `applied: false`. A proposal is not a capability,
reservation, credential or permission to change a live limit.

The owner Unix operation is:

```json
{"version":1,"operation":{"type":"plan_global_resources","id":"<session UUID>","minimum":null,"gpu_device_id":null}}
```

The current daemon supplies an instance-only snapshot and deliberately queues
with `global_accounting_incomplete`; it has no evidence of all other daemon
instances, model servers or GPU jobs. The reply also contains the translated,
validated Person 1 budget. Existing owner resource admission remains scoped to
its actual nested systemd hierarchy.

The eventual global owner must serialize claims across services, bind immutable
request IDs to allocations, subtract all active reservations, apply changes
through one systemd/cgroup writer, and release reservations only after verified
process cleanup. It must publish its accounting scope and pressure provenance.
Agent requests alone cannot attest coverage or expand parent authority.

Tests cover all four decisions, minimum preservation, stale observations,
incomplete accounting and child authority. The live contract smoke verifies the
current owner operation queues without applying resources.

## Shared managed-domain reservations — November 20

`cerynth_resources::authority` now serializes durable root reservations across
participating agentd instances. It uses one private, versioned, atomically
replaced journal and an exclusive file lock per transaction. Claim intent is
fsynced before a local allocation is acknowledged or a process is launched.
The original systemd hierarchy remains the only kernel writer.

This implements P3-087's resource-request design against Person 1's unchanged
budget mechanism. Cross-track agreement is still P3-010. This implementation
does not declare unregistered services accounted for or replace their policies.
The older `plan_global_resources` operation still refuses to attest whole-machine
coverage; actual registered-domain admission uses `resource_admit`.

Initialize an explicit owner policy; for example, a small development domain:

```json
{"version":1,"capacity":{"memory_bytes":805306368,"cpu_quota_micros_per_second":1000000},"inference_limit":4,"accelerators":{}}
```

```sh
cargo build --locked -p cerynth-resources -p cerynth-agentd
cerynth-resource-authority init --directory /absolute/private/authority --policy /absolute/policy.json
CERYNTH_RESOURCE_HIERARCHY=1 CERYNTH_RESOURCE_AUTHORITY_DIR=/absolute/private/authority cerynth-agentd --directory /absolute/private/agent-state serve
cerynth-resource-authority status --directory /absolute/private/authority
```

Every participating daemon selects the same authority directory. Initialization
refuses existing state. Capacity must preserve physical RAM/CPU headroom; root
requests require an explicit CPU quota. A root claim reserves CPU, RAM,
inference concurrency and any selected GPU memory together. Reductions require
the owner's minimum. Subagents remain inside that claim and their existing
kernel hierarchy, rather than being charged to the shared total again.

For the one-GPU reference slice, `accelerators` maps the actual inventory device
ID to `total_bytes` and `desktop_reserve_bytes`. Admission checks fresh device
memory, validates policy against discovered hardware, and labels its reservation
cooperative. Root and descendant GPU job requests are summed at every ancestor;
a parent cannot consume its whole GPU budget while a child independently
consumes that same budget. A job must target its root's reserved device.

The private member binding retains the authority directory, canonical member
identity and policy fingerprint. Omitting the directory environment variable on
restart reuses that binding. Disabling the hierarchy, changing the authority or
silently changing the policy fails closed. The authority path is excluded from
managed workspaces and Guard filesystem grants.
Member preflight runs under the registry lock before process recovery; a copied
member binding cannot cause recovery to stop the original member's live scopes.

A member's disappearance does not expire its claims. Restart first verifies its
recorded process/cgroup cleanup, reconciles incomplete local allocation intent,
and releases only its own drained roots. Pending process, Guard, GPU or model
cleanup retains capacity. For sessions with coordinator grants, shared release
also requires confirmed revocation; an unavailable coordinator retains the claim.
Other members' reservations remain unchanged. A busy transaction is a bounded
retryable error; queued admissions require an explicit retry with the same demand.

This is one trusted Unix owner's reservation domain. Member identities are
consistency checks, not credentials protecting mutually hostile owner services.
Owned model-server processes, direct coordinator administrative grants, ordinary
applications and clients bypassing this domain are excluded. Inference capacity
here means root grant reservations; it is not preemption of backend GPU work.
There is no hardware VRAM partition, cross-user authority, automatic adoption,
policy hot replacement or claim of general performance improvement.

`tools/shared_authority_smoke.py` runs two real daemons, nested processes,
cross-instance CPU/RAM/inference contention, NVIDIA reservation reduction,
parent/child GPU limits and SIGKILL/restart recovery. Rust tests cover concurrent
reservation attempts, immutable identities, stale evidence, retained crash
claims, policy-fingerprint changes and the claim/local-registry crash gap.
