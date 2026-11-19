# Person 3 remaining partials — November 19, 2026

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
