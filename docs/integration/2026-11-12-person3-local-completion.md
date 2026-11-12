# Person 3 local completion audit — November 12, 2026

This continuation closes **13 of the 20 previously partial tasks** with scoped
local implementation and evidence: P3-021, 027, 039, 062, 066, 072, 074, 084,
089, 095, 098, 109 and 110. It does not assert that all 112 roadmap tasks or the
public alpha are complete. The weekly plan and historical Week 1 guide are unchanged.

The [acceptance ledger](../roadmap/person3-tasks.md) remains the current status
source. Team approval, live cloud execution, arbitrary native tool deduplication
and unvalidated cache privacy are not inferred from mocks or successful CLI exits.
The user explicitly deferred cloud tests until last; no provider/account was
selected, no host credential was imported and these fixtures made no cloud model requests.

## Implementation and evidence

| Area | Actual result | Scope and limits |
| --- | --- | --- |
| CPU/RAM admission | Owner accept/reduce/queue/reject, downward-only minimum grants, UUID allocations | One opt-in instance authority, not a replacement for Person 1's global allocator |
| Hierarchical resources | Three real systemd slice levels; 1/.5/.25-core kernel quota configuration, aggregate RAM and sibling reservation bounds | Default Linux scheduler; experimental hierarchy refuses unqualified sched_ext operation |
| Recovery | Leaf-first cascade, SIGKILL/restart releases reservations and scopes; unknown launch effects retained | No automatic replay of unknown external effects |
| Pressure | Fresh kernel samples gate CPU/RAM/GPU/inference, expire if sampling stalls, deny forged data-plane input | Proposed shared contract; Person 1 service stream/review still outstanding |
| Authentication | Native OpenCode/Hermes inventory in private profiles and isolated network namespaces; public key canary/provider name absent from API | Configured inventory is distinguished from a live validated account; host credentials are excluded from probe environment |
| Native Qt client | Real session/event/accounting/GPU/pressure/inference API consumption and screenshot | Read-only observer, not the full Person 4 shell or approval center |
| Model/GPU coordination | Actual Ollama unload, Guard-managed 64 MiB CUDA work, fenced new inference, verified cleanup and lazy reload | Cooperative origin exclusion, no arbitrary-client or hard-VRAM guarantee |
| Residency restart | Durable lease survives coordinator restart; a newly granted client still waits until release | Owned backend dies with coordinator; failure after release is retained rather than treated as successful inference |
| MPS evaluation | Private daemon/client membership verified; 128 MiB client limit permits 32 MiB and rejects 256 MiB; 50% active-thread setting changes reported SMs from 26 to 12 | MIG N/A; no global daemon/compute-mode mutation; no production partition advertised |
| Vendor telemetry | NVIDIA live measurements, Intel discovery, AMD/Intel missing/malformed DRM fixture coverage | AMD physical hardware unavailable; missing telemetry stays null |
| Cross-harness routing | Real OpenCode and Hermes child sessions in separate worktrees, one authenticated inference queue; both output artifact hashes verified | Public workspace copy task, not a general coding success-rate claim |
| Packaging | Exact staged executables/units and native client, isolated user-service dependency startup, automatic restart and recovered process tree | No production enablement, ISO boot, upgrade or signing qualification |

Sanitized evidence is under [docs/benchmarks/person3](../benchmarks/person3/README.md).
Ignored raw artifacts retain fixture output and numerical samples. Reproduction
uses the scripts recorded below and the pinned local binaries/model installations.

## Measurements and regressions

The model-residency comparison alternates engine-default and owner-actuated
CacheWeaver idle-model eviction across three pairs. The model is `qwen3:0.6b`,
Ollama is 0.35.0, and the CUDA payload writes a bounded 64 MiB allocation for two
seconds. Each condition warms the same model before its measured trial.

| Policy | Trials | Mean workflow ms | Model VRAM observed during GPU work |
| --- | ---: | ---: | ---: |
| Ollama keep-alive default | 3 | 2512.00 | 930,401,484 bytes |
| Cerynth idle-model eviction | 3 | 6040.49 | 0 bytes |

Eviction freed the observed model memory and delayed fresh inference until GPU
cleanup, while adding approximately 3.53 seconds to this small-model workflow.
This is a resource/latency tradeoff, not a speedup. The dataset retains every
trial, load duration and queue/request measurements. Model VRAM is Ollama's
`/api/ps` size_vram observation, not a causal measurement of total device memory.
It does not establish
general throughput, energy, KV-cache transfer or memory-pressure performance.

Under a separate one-core parent budget, the two half-core child busy loops had
a maximum/minimum consumed-CPU ratio of about 1.001. Owner status IPC p95 was
about 1.1 ms and Qt offscreen frame interval p95 about 16.5 ms. This is bounded
contention on one laptop, not full-machine saturation or physical desktop input
latency. See the raw samples before drawing broader responsiveness conclusions.

Failed Hermes experiments remain in the published evidence. The default bridge
was called with malformed arguments by the small model. A generated read-only
terminal profile corrected the tool surface, but Hermes correctly blocked the
Python `-c` command without human approval. The accepted routing fixture uses
`cp` within its assigned workspace, preserving native approval behavior. Both
artifacts are independently checked; CLI exit zero alone is never success.

The CPU/RAM fixture also encountered legitimate I/O-pressure admission while
builds were writing heavily. It retries only the explicit `resource_pressure`
queue result with a deadline, retaining ordinary capacity queue/reject assertions.
It does not turn off the pressure policy to make acceptance pass.

## Backend ownership correction

The final crash audit found that the previous process-group-only backend manager
could leave an Ollama server after coordinator SIGKILL. Owned model launches now
record a private UUID systemd-scope journal before spawning, arm a parent-death
signal, and reconcile any retained scope at coordinator restart. A second crash
case keeps model weights loaded and verifies that both the server and GPU runner
are removed. The fixture also checks that the model-residency gate survives restart
and that a new credential cannot bypass it. No owned backend processes remain
after fixture teardown. Cleanup uncertainty remains visible and blocks recovery;
there is no process-name-based kill or automatic unknown work replay.

The new scope has per-backend MemoryHigh 6 GiB, MemoryMax 8 GiB and TasksMax 256.
These are prototype limits, not a global memory reservation or an engine sandbox.
Global resource authority remains P3-087. The existing user service and frozen
scheduler interfaces are unchanged.

## Validation

The workspace regression suite passed **241 tests**, with three upstream/doc
tests ignored. Strict Clippy passes for all eight Person 3 packages and all
targets. Qt builds with warnings treated as errors. Agent schemas are regenerated
from Rust and checked for drift. Existing unrelated upstream warnings and the
declared Rust 1.85 MSRV/dependency inconsistency remain documented; this host uses
Rust 1.99. No scheduler source or frozen runtime contract was changed.

Host fixtures:

```bash
python3 tools/resource_hierarchy_smoke.py
python3 tools/auth_inventory_smoke.py
python3 tools/observer_runtime_smoke.py
python3 tools/resource_responsiveness_smoke.py
python3 tools/mps_capability_probe.py
python3 tools/model_residency_smoke.py --models-directory /absolute/installed-models
python3 tools/packaging_runtime_smoke.py
python3 tools/runtime_version_inventory.py
```

The cross-harness script uses explicit `CERYNTH_OPENCODE_BINARY`,
`CERYNTH_HERMES_BINARY` and `CERYNTH_HERMES_INSTALL` owner overrides, then:

```bash
python3 tools/cross_harness_runtime_smoke.py \
  --models-directory /absolute/installed-models --model qwen2.5:3b \
  --hermes-terminal-profile --simple-copy --require-success
```

The inventory verifies OpenCode 1.19.5, Hermes 0.23.1, Codex 0.167.0,
Claude Code 2.1.312, Ollama 0.35.0 and isolated engine metadata. Version probes
have no external network. Detecting Claude's version does not qualify its
unperformed authenticated cloud workflow.

## Remaining acceptance

Seven of the original partial tasks remain open:

| ID | Concrete remaining work |
| --- | --- |
| P3-019 | Connect an actual Person 1 allocation service/API; the working owner resource foundation is not that absent service |
| P3-045 | Authoritative per-tool broker IDs across native external effects; launch dedup and unknown-outcome fences do not prevent arbitrary tool duplication |
| P3-082 | Validate/enforce broader authorization and external KV-cache isolation; native vLLM per-grant salt tests do not qualify LMCache/SGLang or timing leakage |
| P3-085 | User-deferred live cloud routing test with an explicit dedicated provider/credential variable |
| P3-087 | Team agreement and machine-wide resource authority across services/instances |
| P3-099 | Shared contract compatibility review and stabilization; v1alpha1 remains experimental |
| P3-107 | Complete live compatibility qualification for the cloud-backed features; local pins and unsupported features are now reproducibly inventoried |

P3-094 advances from pending to partial: the pressure contract, kernel sampler and
admission wiring work, but the Person 1 service integration remains unreviewed.
P3-010 still needs actual teammate agreement; P3-105/106 await the deferred cloud
setup and mixed workload qualification; P3-112 still needs release review.

The [review handoff](../contracts/resource-admission-v1alpha1.md) identifies
concrete responsibilities. No messages were sent to teammates, no agreement was
fabricated, and no PR merge, production service enablement or distribution release
was performed.

Primary implementation references: [systemd transient units](https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.systemd1.html),
[Qt local sockets](https://doc.qt.io/qt-6/qlocalsocket.html),
[NVIDIA MPS controls](https://docs.nvidia.com/deploy/mps/appendix-tools-and-interface-reference.html),
[MPS environment limits](https://docs.nvidia.com/deploy/mps/appendix-environment-variables.html).
