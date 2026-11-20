# Experimental resource and model-residency handoff

Status: implemented on the Person 3 feature branch; **not a team-approved stable
contract**. This extends the separate agent API, never the frozen scheduler IPC.
Persons 1, 2 and 4 have a concrete implementation and fixtures to review; no
agreement or approval is implied.

## Owner resource authority

Enable `CERYNTH_RESOURCE_HIERARCHY=1` on one owner agentd instance. The default
managed memory ceiling is half physical RAM; `CERYNTH_AGENT_MEMORY_MAX_BYTES`
can select a lower owner ceiling while retaining at least 512 MiB of physical
headroom. The CPU ceiling leaves two logical CPUs outside the managed quota
where available. These are capacity policies, not dedicated physical-core
reservations. Multiple independently configured daemons are not a machine-wide
allocator. The [shared-domain handoff](global-resource-request-v1.md#shared-managed-domain-reservations--november-20)
now adds an opt-in common reservation journal across participating instances.
Unregistered model servers and bypassing clients remain outside that domain;
cross-track ownership and complete machine accounting are not inferred.

The experimental hierarchy requires sched_ext disabled until Person 1 qualifies
the scheduler's CPU policy. No scheduler is stopped or replaced by this feature.
The validated host used the default Linux scheduler. `cpu_weight` remains
relative preference; an optional `cpu_quota_micros_per_second` specifies quota
per second, in multiples of 10,000 (500,000 means half one logical CPU).

All requests use the existing private owner Unix socket and version 1 envelope:

```json
{"version":1,"operation":{"type":"resource_admit","id":"SESSION_UUID","minimum":null}}
```

`resource_admit` is required before launching when the feature is enabled.
It returns `accept`, `reduce`, `queue` or `reject`. A reduction needs an explicit
owner `minimum` budget; no agent demand increases its allocation. The admitted
budget replaces the requested budget only downward. An allocation has its own
UUID, parent session identity, granted budget, active flag and kernel-setup flag.
The flag records verified systemd/kernel configuration; it is not qualification
of an arbitrary sched_ext scheduler or hardware GPU partition.

Active root reservations, including terminal sessions with unfinished cleanup,
count against the instance ceiling. Direct child reservations count against
their parent, including CPU quotas. Descendants are nested in actual systemd
slices. Their aggregate memory and CPU activity is bounded by the ancestor's
kernel configuration. Parent work also consumes that aggregate capacity; the
parent does not receive additional capacity outside its subtree ceiling.

The hierarchy is an instance slice, hashed project slice, UUID session slice and
existing supervised process scope. systemd D-Bus owns the settings. No component
writes competing cgroup control files. Journaled slice creation precedes launch;
recovery tears down the exact owned scopes/slices and preserves unknown effects.
Allocation release is leaf first and waits for process/GPU cleanup.

`resource_view` returns versioned capacity/pressure observations and allocations.
`resource_accounting {id}` returns aggregate kernel memory, CPU, PID, OOM and PSI
counters for that session slice. Missing counters stay unknown. The native
observer consumes these operations without gaining a management action in its UI.

## Pressure input

`cerynth_resources::pressure::Sample` is a version 1 read-only kernel sample with
observation time, MemAvailable and memory/CPU/I/O PSI. The owner-only coordinator
operation is `pressure {sample}`. Samples older than five seconds, excessively
future samples and malformed percentages are rejected. Accepted input expires;
a stalled sampler cannot leave admission indefinitely open.

With the hierarchy enabled, agentd sends a fresh sample each second. New CPU/RAM,
GPU and inference work is queued under the documented conservative thresholds.
Executing work is not forcibly killed by a pressure observation. Unit and HTTP
tests cover stale input, forged data-plane input, blocked admission and release.
The pinned Person 1 library now supplies actual cgroup usage/PSI, conservatively
merged with machine headroom before forwarding. Contract approval remains open.

## GPU/model exclusion

An owner can call `gpu_quiesce_model {id, lease_id, endpoint_id, model}` for a new
Generic PTY session with Guard, `gpu.execute` and `model.release`. Use the exact
installed model identifier. The coordinator currently actuates **Ollama only**.
It refuses active/queued inference, durably journals an origin-bound lease before
unloading, and requires `/api/ps` to confirm that exact model is absent.

The session cannot launch its GPU payload with an incomplete or released model
binding. New inference for that backend waits until supervised GPU/process
cleanup and a coordinator release acknowledgement. Owner `gpu_finish` cannot
release a supervised process lease. Coordinator `release_model` independently
checks that the exact systemd GPU scope contains no processes. Incomplete intent
and successful unload both retain the gate across coordinator restart. There is
no automatic expiry that could reload weights onto a still-running GPU workload.

`plan_cache` is an owner-only bounded advisory operation over existing CacheWeaver
metadata. The experiment supplies an explicit long-tool phase, applies an eviction
recommendation through the session-bound operation, then permits lazy reload.
It does not transfer KV tensors, change LMCache or preempt arbitrary CUDA contexts.
There are at most 64 pending/executing data-plane requests, including residency
waiters; revocation releases their slots. Queue metrics include the residency wait.

Backend-origin fencing is cooperative. Another trusted owner process can bypass
the coordinator or change system policy. VRAM reservations are not hardware
partitions. Health exposes held model leases and pending/executing request counts;
the native observer shows those separately from missing measurements.

## Review boundaries

| Track | Concrete review surface | Outstanding decision |
| --- | --- | --- |
| Person 1 | Pinned budget/usage mechanism, shared-domain authority, nested quota/restart and fairness fixtures | Cross-track domain ownership, backend-wide coverage, sched_ext qualification and release integration |
| Person 2 | Guard-mounted tool-effect journal, private-cache mode, owner-only resource/model grants | Shared trust-boundary review and unsupported engine/side-channel qualification |
| Person 4 | Cursor events, resource/accounting/GPU APIs and native read-only Qt consumer | Shared event/desktop contract approval and full physical desktop evaluation |

Implementation references: [systemd D-Bus API](https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.systemd1.html),
[cgroup v2](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html),
[local validation report](../integration/2026-11-12-person3-local-completion.md).

## Owned inference backend recovery

Owned Ollama launch requires the persistent coordinator service and user systemd.
`backend-scopes.json` records bounded private UUID scope identities before spawn.
A parent-death wrapper kills the owned server when its coordinator disappears;
restart reconciles the entire previous scope, including retained GPU runners,
before serving new work. Failed cleanup remains journaled, visible in health and
fail-closed on recovery. Normal stop verifies the scope is empty before discarding
its identity. Backends are not automatically restarted or adopted after a crash.
Per-backend 6/8 GiB high/max memory and 256-task limits are prototype bounds, not
a machine-wide allocation. Configuring an existing endpoint does not take process
ownership or inherit these guarantees.
