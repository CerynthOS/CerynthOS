# Person 3 remaining partials — November 19, 2026

The November 19 snapshot below is historical. The November 20 follow-up at the
end records the shared-domain implementation; the current ledger is authoritative.

Four more partial tasks now have scoped implementation and local acceptance:
**P3-019, P3-045, P3-082 and P3-094**. The current
[ledger](../roadmap/person3-tasks.md) is **104 implemented, 4 partial, 4 pending**.
The weekly plan, scheduler sources and frozen scheduler IPC are unchanged.
No cloud credential, provider account or teammate approval was inferred.

## Completed local integrations

| Task | Implementation | Actual acceptance |
| --- | --- | --- |
| P3-019 | Person 1's actual `ResourceBudget` v1 implementation pinned at `648c149be39f5ea8df612b3266857472976238eb`; unchanged Rust source in the renamed `cerynth-platform-resources` package | Three real nested systemd cgroups: translated/validated requests, typed applied status, usage, pressure and descendant PID containment; systemd remains the only writer |
| P3-045 | Durable intent-before-dispatch journal on the actual sandbox-mounted Guard session socket; immutable tool UUID/details and approval binding | Real pinned Guard and temporary local bare Git remote; independent hook counts one push across duplicate/concurrent requests and daemon restart; separate lost-acknowledgement fault test never resubmits an unknown effect |
| P3-082 | Opt-in `managed_process` cache privacy: distinct session/authorization process generations and private backend homes; unsupported external cache engines fail closed | Real owned Ollama servers, cross-session/alias/cache-forgery denial, authorization reset only after verified scope cleanup, new HOME on reset, strict policy survives omitted environment flag, explicit downgrade rejected |
| P3-094 | Actual Person 1 cgroup PSI merged conservatively with machine headroom and forwarded through the trusted inference control API | Live nested cgroup/coordinator fixture; a typed high-memory-PSI regression blocks admission despite healthy machine headroom; missing owned boundary cannot become zero pressure |

The resource prototype was found on `origin/week1/scx-backend`, rather than main.
Earlier audits that described the Person 1 API as absent were incomplete.
Only the package name is adapted; five source hashes and the upstream revision
are recorded in [provenance](../../crates/cerynth-platform-resources/PROVENANCE.json).
Importing the mechanism does not claim agreement on a machine-wide allocator.

Tool effects are at-most-once submissions through supported Guard mediation.
An unknown effect requires reconciliation, not a new-ID automatic retry.
Unbrokered shell effects, exactly-once delivery and automatic rollback are outside
the guarantee. The private cache mode requires the supported Guard native
inference boundary with host networking denied. It trusts the owner-selected
native Ollama executable and does not establish statistical timing isolation,
arbitrary-engine safety or protection from an unisolated host process.

## Further work made reviewable

P3-087 now has a [typed global demand/snapshot/proposal design](../contracts/global-resource-request-v1.md)
and an executable CPU/RAM/GPU/inference planner. It preserves minimums and parent
authority, uses fresh trusted observations, and distinguishes accept/reduce/
queue/reject. The live daemon honestly reports instance-only accounting and
queues global proposals without actuation. Coordinated ownership and a single
machine-wide authority remain outstanding; a planner is not an allocation service.

P3-099 now has a [content-addressed candidate contract](../contracts/agent-compatibility-candidate.md),
optional server-checked request pinning before mutation, regenerated schemas, and actual
previous-checkpoint Rust request/session fixtures. A live incorrect-pin attempt
cannot create a session through the client or a direct raw frame; the matching
pin succeeds. The check is atomic with dispatch, avoiding a daemon-restart gap.
The previous checkpoint's
records round-trip, preserving unknown outcomes and absent optional fields.
The API remains a candidate pending coordinated compatibility review.

## Remaining acceptance

| Status | Tasks | What remains |
| --- | --- | --- |
| Partial | P3-085, P3-107 | Consented live cloud routing and authenticated provider/version qualification; deferred until last at the user's request |
| Partial | P3-087, P3-099 | Coordinated global-resource ownership and shared API compatibility/stabilization review |
| Pending | P3-010 | Actual agreement with Persons 1, 2 and 4 |
| Pending | P3-105, P3-106 | Authenticated mixed-harness and five-agent local/cloud reference runs |
| Pending | P3-112 | Release integration/regression review and release acceptance |

No messages have been sent to teammates. A local test, type or proposed contract
is not substituted for their review. Live cloud execution still needs an
explicit provider endpoint and dedicated credential configuration; no ambient
subscription credential is used.

## Validation and reproduction

The workspace suite passed **268 tests**, with **3 ignored** upstream/doc tests.
The expanded pure-planner cases also pass separately. Strict Clippy passes for
the eight Person 3 packages and all their targets. Workspace all-target checking,
formatting, legacy compatibility, generated-schema drift checks and source-hash
verification pass. The pinned production platform library is checked as a
dependency; its 19 original unit tests pass in the workspace. Its two original
test assertions trigger a newer Clippy lint under a separate strict platform-test
invocation, so the original source is preserved and that limitation is explicit.

The first broader test run exposed a scheduler reporter timing failure; its
isolated rerun and later full workspace runs passed without scheduler changes.
A subsequent run passed its unit/integration groups but failed upstream doctest
linking when disk filled. Obsolete generated test executables and incremental
build data were removed; the complete suite then passed. An existing Guard smoke
test caught an error-code compatibility regression in the new gateway; restoring
the pinned `caller_not_authorized` denial fixed it. Failed logs are retained in
the ignored validation artifacts alongside accepted runs.

Run from the repository root with Rust/Cargo and the pinned Guard executables
available; resource/owned-backend fixtures require a working user systemd manager
and this host's qualified cgroup configuration:

```sh
cargo test --locked --workspace
scripts/check-agent-contracts.sh
python3 tools/contract_compatibility_smoke.py
python3 tools/tool_effects_smoke.py
python3 tools/resource_hierarchy_smoke.py --output artifacts/person3/platform-resources-smoke.json
python3 tools/cache_privacy_smoke.py --models-directory /absolute/path/to/installed/models
python3 tools/guard_runtime_smoke.py --output artifacts/person3/guard-runtime-smoke-nov19.json
python3 tools/native_guard_runtime_smoke.py --output artifacts/person3/native-guard-runtime-smoke-nov19.json
python3 tools/model_residency_smoke.py --models-directory /absolute/path/to/installed/models --output artifacts/person3/model-residency-smoke-nov19.json
python3 tools/publish_person3_evidence.py
```

The real cache fixture uses installed `qwen3:0.6b` with Ollama 0.35.0. The Guard
integration remains pinned to `e592fa47797179ad24a99fc76143be48191a7649`.
The model-residency regression retains all three paired trials and both unloaded
and loaded-backend crash recovery checks. These are host-specific enforcement
and cleanup checks, not a new performance or public-alpha readiness claim.
Sanitized reports and raw-artifact SHA-256 identifiers are under
[measured evidence](../benchmarks/person3/README.md).

## November 20 shared-domain and compatibility follow-up

P3-087's technical resource-request design is now implemented with a shared
managed-domain mechanism against Person 1's unchanged budget contract. The
current ledger is **105 implemented, 3 partial, 4 pending**. This closes the
technical design scope, not Person 1 agreement or complete workstation accounting.
Those review and platform-release decisions remain separately tracked.

The new `cerynth-resources::authority` library and `cerynth-resource-authority`
owner helper provide one private, versioned, serialized reservation journal.
Two explicitly participating agentd instances now reserve CPU, RAM, inference
concurrency and GPU estimates against the same capacity. Claims are durable
before allocation acknowledgement; children stay inside their root claim and
actual nested systemd hierarchy. Root GPU requests and descendants cannot
independently consume the same reserved budget, and jobs must use the reserved
device. GPU reservations remain cooperative.

SIGKILL does not expire a member's claim. Recovery verifies owned scope cleanup
before releasing that member's roots. An allocation-intent crash gap is
reconciled without replaying work; uncertain process/Guard/GPU/model cleanup or
unconfirmed inference revocation retains capacity. Archival cannot discard
unconfirmed credential evidence. Saved member and policy fingerprints prevent
restart from silently switching domains or disabling the hierarchy. Managed
workspaces and Guard filesystem grants cannot expose the authority directory.
Observed processes cannot inherit managed allocations.
Member validation also precedes process recovery under the registry lock; the
live copied-member test verifies the original process remains alive and Running.

`shared-authority-smoke.json` records two real daemons, nested processes,
cross-instance memory and independently isolated CPU contention, inference
reservation contention, actual NVIDIA telemetry, GPU grant reduction, subtree
GPU limits, real Guard denial/allowed execution and SIGKILL recovery. The
packaging fixture stages the exact six Rust executables, native observer,
desktop entry, documentation and two units; it exercises the installed authority
helper and isolated user-service restart without enabling production units.

The protocol candidate now has a committed manifest exported by
`cerynth-agent-schema --manifest`. CI compares it alongside both generated JSON
schemas. The behavior revision and feature fingerprint changed intentionally;
old pins fail before mutation and ordinary unpinned legacy frames still work.
No stable shared API or teammate approval is claimed.

The workspace suite passes **272 tests**, with **3 ignored** upstream/doc tests.
Strict Clippy passes for all eight Person 3 packages and their targets. The
workspace all-target check, formatting, shell checks and three-part contract
drift check pass. Independent reservation clients, concurrent attempts,
substitution, retained crash claims, stale observations, changed policy,
unconfirmed revocation and allocation-intent recovery have additional Rust
regressions. The default instance-only hierarchy fixture still passes.

The extended fixture initially expected an early rejected launch to release its
explicitly admitted Created session automatically. The reservation correctly
remained until owner cancellation; the fixture now verifies and cancels it.
The packaging fixture's old fixed file count also needed replacement with an
exact expected payload and source-hash checks for the new helper. Both earlier
failures and accepted reruns are retained in the raw validation artifacts.

Reproduction adds:

```sh
python3 tools/shared_authority_smoke.py
python3 tools/contract_compatibility_smoke.py --output artifacts/person3/contract-compatibility-smoke-nov20.json
python3 tools/resource_hierarchy_smoke.py --output artifacts/person3/platform-resources-smoke-nov20.json
python3 tools/packaging_runtime_smoke.py --output artifacts/person3/packaging-runtime-smoke-nov20.json
shellcheck -S warning scripts/check-agent-contracts.sh scripts/install-agent-runtime.sh
```

The shared mechanism accounts only for participating agentd root subtrees.
Unregistered model-server processes, direct coordinator administrative grants,
ordinary applications and bypassing clients are outside that domain. It does
not introduce cross-user isolation, hardware VRAM partitions, backend preemption,
automatic adoption or live policy replacement. See the
[owner setup and precise scope](../contracts/global-resource-request-v1.md#shared-managed-domain-reservations--november-20).

The remaining acceptance is P3-085/P3-107 and P3-105/P3-106 live cloud/mixed tests,
plus P3-010/P3-099/P3-112 coordinated agreement, stabilization and release review.
The dedicated provider configuration is still missing; cloud requests remain
zero. No messages have been sent to teammates. The weekly plan, scheduler and
frozen scheduler IPC remain unchanged.
