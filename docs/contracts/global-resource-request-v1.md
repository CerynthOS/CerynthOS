# Global resource request candidate v1

This is the P3-087 design and executable planner, paired with Person 1's actual
[ResourceBudget v1](resource-budget-v1.md) at commit
`648c149be39f5ea8df612b3266857472976238eb`. Team agreement remains P3-010.
One machine-wide allocation authority is still required before this planner can
be treated as global admission.

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
