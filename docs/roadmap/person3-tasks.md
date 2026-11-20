# Person 3 task acceptance ledger

Audit date: 2026-11-20. **All 112 tasks are not complete.** The weekly plan and
historical Week 1 guide are unchanged. This ledger separates implemented local
functionality from partially implemented integrations and outstanding acceptance.

**Implemented** means the described scoped functionality exists with the stated
evidence; it does not mean team approval or production readiness. **Partial**
identifies concrete missing behavior or validation. **Pending** has no completed
acceptance result. No teammate agreement, cloud execution, hard GPU partitioning,
or unperformed benchmark is inferred from a type, placeholder, or process exit.

Current audit: **105 implemented, 3 partial, 4 pending**.
These are task-scope labels, not production-readiness percentages.

Evidence references:

- [November 20 shared-domain follow-up](../integration/2026-11-19-person3-remaining-partials.md#november-20-shared-domain-and-compatibility-follow-up), [resource design and owner setup](../contracts/global-resource-request-v1.md#shared-managed-domain-reservations--november-20).

- [November 19 remaining-task audit](../integration/2026-11-19-person3-remaining-partials.md), [tool effects](../runtime/tool-effects.md), [private cache mode](../runtime/cache-privacy.md), [candidate compatibility](../contracts/agent-compatibility-candidate.md).

- [November 12 local completion audit](../integration/2026-11-12-person3-local-completion.md), [resource handoff](../contracts/resource-admission-v1alpha1.md), [native observer](../runtime/native-observer.md).

- [Runtime guide](../runtime/agent-runtime.md), [architecture](../architecture/person3-runtime.md), [schema](../contracts/agent-session-v1alpha1.md).
- [Harness/protocol/hardware research](../research/person3-interfaces.md).
- [Validation and experiment report](../integration/2026-11-11-person3-runtime.md).
- `services/cerynth-agentd/tests`: real processes, PTY, restart cleanup, persistence, authority bounds, sharing and artifact safety.
- `services/cerynth-inferenced/tests`: HTTP streaming/authentication, queues, revocation, model allowlists and privacy boundaries.
- `crates/cerynth-model-api`: inference clients, hierarchical admission, cache-policy tests.
- `crates/cerynth-resources`: discovery and cooperative GPU admission tests.
- `crates/cerynth-agent-protocols`: negotiated ACP client and bounded OpenCode HTTP client.

## Phase 0 — Research and architecture

| ID | Task | Status | Evidence / remaining acceptance |
| --- | --- | --- | --- |
| P3-001 | Study Rust services, IPC, configuration and telemetry | Implemented | Architecture study traces existing services and preserves frozen scheduler contract. |
| P3-002 | Research OpenCode, Hermes, Codex and Claude Code interfaces | Implemented | Primary-source matrix; pinned executable discovery and local experiments. |
| P3-003 | Research ACP, MCP, A2A and compatible inference interfaces | Implemented | Versioned role matrix and explicit protocol boundaries. |
| P3-004 | Harness compatibility matrix | Implemented | Lifecycle, API, tooling, subagents and actual Cerynth support separated. |
| P3-005 | Inventory CPU, GPU, VRAM, drivers and inference | Implemented | Real host inventory; unavailable metrics remain unknown. |
| P3-006 | Select initial harness and local backend | Implemented | OpenCode 1.19.5 + Ollama 0.35.0; real managed coding task independently verified. |
| P3-007 | AgentSession v1 ownership/config/resource schema | Implemented | Rust contract and generated JSON Schema, independent of scheduler IPC. |
| P3-008 | Lifecycle states and transitions | Implemented | Validated transition table; terminal states cannot resurrect. |
| P3-009 | Capability-negotiated adapter API | Implemented | Common preparation trait used by Generic PTY and all four native CLI drivers; explicit capability omissions and shared supervisor ownership; contract tests. |
| P3-010 | Agree interfaces with Persons 1, 2 and 4 | Pending | Proposed handoffs documented; actual team review/approval has not happened. |

## Phase 1 — Core agent runtime

| ID | Task | Status | Evidence / remaining acceptance |
| --- | --- | --- | --- |
| P3-011 | Create cerynth-agent-api | Implemented | Workspace crate with versioned requests, records and validation. |
| P3-012 | Create cerynth-agentd | Implemented | Owner Unix API, supervisor, bounded workers and private persistent registry. |
| P3-013 | Registry with stable UUIDs | Implemented | Atomic/fsynced state, exclusive lock, archival, corruption rejection. |
| P3-014 | Create/start/list/status/input/stop APIs | Implemented | CLI and Unix operations; actual process and PTY tests. |
| P3-015 | Generic CLI/PTY adapter | Implemented | portable-pty transport with supervised ownership; unsupported semantic features explicit. |
| P3-016 | First real harness | Implemented | OpenCode managed launch and independently verified local coding smoke task. |
| P3-017 | Real local inference endpoint | Implemented | Ollama native streaming and authenticated compatible proxy exercised. |
| P3-018 | Process trees, exits and failures | Implemented | Boot/PID-generation identity, descendants, exit codes, deadlines and failure reasons. |
| P3-019 | Connect Person 1 resource API | Implemented | Actual Person 1 ResourceBudget v1 library pinned at 648c149, source hashes preserved; adapter validates budgets and reads typed status/usage/descendant PIDs in three real systemd levels. systemd remains the sole writer. Team approval stays P3-010; global authority stays P3-087. |
| P3-020 | Connect Person 2 sandbox/authorization API | Implemented | Pinned real Guard sandbox/authorization, offline inheritance and opt-in per-root native inference identity bridge exercised. Model coding failures retained; native delegation/tool-effect dedup remain separately limited. |
| P3-021 | Publish structured events for Person 4 | Implemented | Cursor session/tool/inference events preserve provenance and retention gaps; actual native Qt owner-API consumer and live fixture now exercise them. Team contract approval remains P3-010/P3-099. |
| P3-022 | Startup/termination/cleanup/crash tests | Implemented | SIGKILL/restart, orphan cleanup, parent exit, timeout, PTY and shutdown regressions. |

## Phase 2 — Universal harness integration

| ID | Task | Status | Evidence / remaining acceptance |
| --- | --- | --- | --- |
| P3-023 | OpenCode supported-API adapter | Implemented | Owned password-protected OpenCode HTTP server wired to create/prompt/abort/history with intermediate tool normalization; actual managed API coding smoke and contract tests. |
| P3-024 | Hermes adapter | Implemented | Pinned 0.23.1 launches with private home/read-only dependencies, structured tools and brokered local inference; small-model coding failures recorded. |
| P3-025 | Codex or Claude Code adapter | Implemented | Codex 0.167.0 local execution recipe exercised; Claude recipe detected but authenticated cloud workflow unverified. |
| P3-026 | Installation and runtime-version detection | Implemented | Owner-selected executable resolution and bounded version probes. |
| P3-027 | Secret-free authentication checks | Implemented | Bounded native auth inventories return only state; OpenCode/Hermes private-profile network-isolated canary fixture passes. Configured credentials, unknown state and live authenticated account status remain distinct; no ambient credential environment forwarding. |
| P3-028 | Structured IO/event streams | Implemented | Native JSONL output and transparent inference SSE; unsupported formats stay raw private output. |
| P3-029 | Normalize tools/results/usage/errors | Implemented | Supported native JSONL tools/results/errors, nonzero command exits, token usage and cache read/write fields normalized with regression fixtures; unknown formats remain private raw output. |
| P3-030 | Honest cancellation/resumption support | Implemented | Owned-process cancel supported; native resume not advertised; ACP load only when negotiated. |
| P3-031 | Discover existing sessions | Implemented | Configured authenticated OpenCode API discovery live-tested; same-owner PID observation; no automatic import or sandbox/control claim. Other native discovery is unsupported. |
| P3-032 | Managed/integrated/observed modes | Implemented | Boundary evidence persisted; observed processes never killed or falsely sandbox-labelled. |
| P3-033 | ACP-compatible agents | Implemented | Supervised OpenCode ACP v1 execution, streamed updates, capability negotiation and denied host operations; real pinned harness exercised, no universal ACP/native-resume guarantee. |
| P3-034 | Adapter contract tests/unsupported features | Implemented | Shared CLI/PTY adapter contract tests, event fixtures, ACP negotiation/denial tests and explicit unsupported-feature matrix. Live provider/version validation remains separately limited. |

## Phase 3 — Multi-agent orchestration

| ID | Task | Status | Evidence / remaining acceptance |
| --- | --- | --- | --- |
| P3-035 | Parent/child identities and relationships | Implemented | Persisted parent/task IDs, bounded ancestry and graph edges. |
| P3-036 | Task assignment/delegation API | Implemented | Objective/input/output/completion contract and owner-sponsored child creation. |
| P3-037 | Bounded child creation | Implemented | Deadlines, max children, individual and aggregate reservations validated. |
| P3-038 | Person 2 inheritance checks | Implemented | Pinned real Guard evaluates registered parent/child UUIDs and rejects increased child capability; local subset checks and leaf-first authorization cleanup also tested. |
| P3-039 | Person 1 hierarchical accounting | Implemented | Real nested systemd slices enforce aggregate parent limits on the qualified host; direct sibling CPU reservations are bounded, kernel usage includes descendants, cascade/SIGKILL recovery verified. Person 1 global API/review remains separate. |
| P3-040 | Messaging and structured results | Implemented | Explicit sharing authorization, scoped bounded messages and task-result artifacts. |
| P3-041 | Graphs/dependencies | Implemented | Delegation/dependency edges; unresolved dependencies block launch. |
| P3-042 | Git worktrees/workspace mapping | Implemented | Retained per-agent branch/worktree; hooks disabled, filters disabled in network-denied scoped checkout; durable preparation journal and real hostile-hook/filter smoke test. |
| P3-043 | Child artifacts/diffs/outputs/tests | Implemented | Owner collector snapshots child candidate/output, creates immutable diff and test receipts, runs an independent managed checker and detects mutation/symlinks; passing/failed checks tested. Assertions are explicitly owner-selected, not general correctness proof. |
| P3-044 | Cancellation/deadlines/bounded retries | Implemented | Recursive cancellation/deadlines plus three bounded owner-sponsored retries with atomic UUID links, complete-cleanup/known-outcome checks and restart deduplication. No automatic tool replay. |
| P3-045 | Prevent duplicate external effects | Implemented | Durable at-most-once submission through the sandbox-mounted actual Guard tool socket; immutable UUID/details, lost-ack fault recovery, approval binding, concurrent duplicate and real local Git-push count verified. Arbitrary unbrokered shell effects are outside this guarantee. |
| P3-046 | Concurrent agents/termination/workspace tests | Implemented | Real parent-child termination and active-workspace conflict regression tests. |

## Phase 4 — Inference infrastructure

| ID | Task | Status | Evidence / remaining acceptance |
| --- | --- | --- | --- |
| P3-047 | Model API/coordinator interfaces | Implemented | Typed local/compatible clients, admin/data contracts and admission API. |
| P3-048 | cerynth-inferenced | Implemented | Separate owner/data sockets and optional token-authenticated loopback HTTP. |
| P3-049 | Supported Ollama integration | Implemented | Inventory/show/ps/chat/unload and compatible tool-stream forwarding exercised. |
| P3-050 | Model registry/backend/quantization/context | Implemented | Bounded persistent normalized registry: backend/model/digest, quantization, architecture vs active context, weight vs allocated memory/VRAM; restart residency invalidation and tests. |
| P3-051 | Start/stop/health/readiness | Implemented | Explicitly owned backend processes, bounded readiness, failure cleanup and health API. |
| P3-052 | Endpoint discovery/registration | Implemented | Owner registration persists; bounded explicit numeric-loopback discovery reports reachability without automatic registration or LAN scanning. |
| P3-053 | Consented cloud endpoint support | Implemented | HTTPS + endpoint/session consent, service-only credential environment reference; policy tests make no cloud calls. Live provider interoperability remains unverified. |
| P3-054 | Session/request/usage correlation | Implemented | Session/endpoint/model/trace metrics and proxy response correlation headers. |
| P3-055 | Authenticated shared inference | Implemented | Random per-session tokens, hashed storage, model allowlist, revocation, private admin plane. |
| P3-056 | Queue/concurrency/priority | Implemented | Bounded priority/FIFO queue with an eight-bypass starvation bound; session/ancestor/project/global limits and cancellation-safe leases; regression tests. |
| P3-057 | TTFT/rate/queue/p50/p95 | Implemented | Actual streaming content timing, engine counters, bounded metric samples; unsupported measurements remain null. |
| P3-058 | Failures/cancel/backpressure/timeouts | Implemented | End-to-end deadline, output bounds, revoked queue/stream tests and explicit outage errors. |
| P3-059 | Model/privacy isolation tests | Implemented | Unauthorized model/token rejection, sticky origin trust domains, restart token rotation and cloud consent tests. Engine-level isolation still needs backend validation. |
| P3-060 | Shared versus separate model benchmark | Implemented | Three warm trials per setup, separate cold results, actual model/VRAM observations; narrow single-host synthetic result only. |

## Phase 5 — GPU and VRAM orchestration

| ID | Task | Status | Evidence / remaining acceptance |
| --- | --- | --- | --- |
| P3-061 | GPU discovery/capability matrix | Implemented | NVIDIA and DRM discovery; actual host support/enforcement matrix published. |
| P3-062 | Vendor GPU utilization/memory adapters | Implemented | NVIDIA live telemetry, Intel discovery and injectable AMD/Intel DRM utilization/memory adapters; missing/malformed sysfs tests preserve unknowns. AMD physical hardware remains unverified and is labelled accordingly. |
| P3-063 | Supported workload/process GPU usage | Implemented | Supported NVIDIA compute-process memory; graphics and missing metrics explicitly limited. |
| P3-064 | GPU requests/device/memory/priority | Implemented | Typed request and validated cooperative budget. |
| P3-065 | GPU admission/queue/reject | Implemented | Capacity, desktop margin, priority/FIFO and expiry tests. |
| P3-066 | Model residency versus GPU jobs | Implemented | Actual Ollama unload coordinated with Guard CUDA job, durable origin fencing, bounded waiters, restart recovery and lazy reload after independently verified GPU-scope cleanup. Cooperative, Ollama-only actuation; no arbitrary-client isolation claim. |
| P3-067 | Distinguish enforcement levels | Implemented | Monitoring/cooperative labels; no unsupported hard GPU guarantee. |
| P3-068 | Cooperative VRAM accounting | Implemented | Reservation release/expiration and session budget validation. |
| P3-069 | Desktop GPU capacity | Implemented | Default 1 GiB admission safety margin; cannot constrain bypassing processes. |
| P3-070 | GPU lifecycle/cleanup/timeouts/failure | Implemented | Actual Guard/systemd CUDA job execution; immutable job/operation binding, timeout/cleanup and retained lease until verified scope emptiness. Manual mid-process release denied. |
| P3-071 | GPU authorization | Implemented | Actual Guard GPU allow/deny decision bound to job and launch hash, plus authoritative agentd launch gate. Guard decision label remains advisory; no hard VRAM claim. |
| P3-072 | GPU UI allocations/queue | Implemented | Native Qt client consumes real admitted/queued GPU jobs, device memory and resource state through owner APIs; live screenshot fixture passes. Full Person 4 shell remains separate. |
| P3-073 | CUDA/Python versus local inference | Implemented | Bounded real CUDA bandwidth fixture overlaps real Ollama requests; smoke experiment, no hard-isolation/performance claim. |
| P3-074 | Evaluate MIG/MPS where available | Implemented | MIG N/A recorded; private real MPS daemon/client test validates 128 MiB client cap and 50% thread setting with actual CUDA allocations/SM observations. Cooperative evaluation only; no generic hard partition advertised. |

## Phase 6 — CacheWeaver and optimization

| ID | Task | Status | Evidence / remaining acceptance |
| --- | --- | --- | --- |
| P3-075 | Loading/context-recompute baseline | Implemented | Real cold load, three fresh/repeated pairs at each of three prefix sizes, and six model switches; full engine counters retained without claiming verified cache hits. |
| P3-076 | Evaluate vLLM/SGLang | Implemented | Pinned Python 3.12 environments ran vLLM 0.25.0 and SGLang 0.5.16 on this GPU, matched checkpoint/native prefix enabled-disabled trials, real hit counters, failures and cleanup retained. |
| P3-077 | Research native KV/prefix caching | Implemented | Primary-source engine study covers vLLM, SGLang and LMCache prefix/KV/offload controls, trust boundaries and version-specific limitations; no invented engine API. |
| P3-078 | Evaluate LMCache offload/reuse | Implemented | LMCache 0.5.6 external connector performed bounded CPU offload and three 256-token restores with native prefix caching disabled. Native wheel ABI mismatch/fallback and transfer overhead are documented; not production qualification. |
| P3-079 | Session residency/cache-hit data | Implemented | Session-correlated before/after typed model residency and native reported cache-token dataset retained in harness-trials.json; lower-trust counters labelled, not independently verified hits. |
| P3-080 | Retain/evict/offload/recompute policy | Implemented | Metadata policy compares costs, protects active/interactive entries and requires actual offload capability. |
| P3-081 | Experimental session-aware cache policy | Implemented | Deterministic recommendation policy and tests; it does not execute KV transfers. |
| P3-082 | Prevent cross-tenant cache leakage | Implemented | Opt-in managed_process mode requires one owned native Ollama process generation/private HOME per session authorization realm; unsupported external KV engines and aliases fail closed. Actual local reset/restart/downgrade checks and launcher boundary regressions. Not a timing-side-channel proof; default shared mode remains less restrictive. |
| P3-083 | Placement/switching-cost estimates | Implemented | Owner-only advisory placement planner uses fresh device telemetry, authorization, calibrated resident memory and actual switching samples; protects active models and retains outliers. It does not actuate placement. |
| P3-084 | Compare engine defaults | Implemented | Three alternating pairs compare an actuated idle-model eviction recommendation with Ollama defaults: model VRAM released, reload latency regressed. Raw samples and limitations retained; no KV transfer or general speedup claim. |
| P3-085 | Consented local/cloud routing evaluation | Partial | Explicit consent enforced; authorized live cloud routing experiment not run. |
| P3-086 | Publish benchmark/regressions | Implemented | Sanitized numerical samples, cold outliers, CUDA smoke limits, failed coding trials and the validation report are retained in docs/benchmarks/person3; advanced engine comparison remains P3-084. |

## Phase 7 — Resources and interoperability

| ID | Task | Status | Evidence / remaining acceptance |
| --- | --- | --- | --- |
| P3-087 | Global resource semantics with Person 1 | Implemented | Technical design uses unchanged P1 budgets plus an opt-in shared root-reservation authority across agentd instances. Live CPU/RAM/inference/GPU contention, bounded subtrees, durable claims, Guard protection and crash recovery verified. Registered-domain scope only; model-server processes/direct grants are outside it. Team agreement remains P3-010. |
| P3-088 | Agent demand/priority signals | Implemented | Validated resource requests, CPU weights, inference priority grants and phase reports. |
| P3-089 | Accept/reduce/queue/reject decisions | Implemented | Owner CPU/RAM admission implements accept/reduce/queue/reject with explicit minimums and no upward grants; root/child reservations and live kernel limits tested. Global Person 1 service integration is separate. |
| P3-090 | Project/parent inference budgets | Implemented | Project, ancestor and session scopes share cancellation-safe coordinator admission. |
| P3-091 | Request additional CPU/RAM/GPU | Implemented | Owner API records validated demand and returns applied:false; no self-approved increase. |
| P3-092 | Prevent self-authorized higher limits | Implemented | Root grants restricted to owner plane; child subsets and immutable inference grants checked. |
| P3-093 | Workload phase reports | Implemented | Inference/tool/compile/idle/approval-wait plus cleanup fields; no CPU heuristic pretends semantic certainty. |
| P3-094 | Person 1 pressure to admission | Implemented | Actual pinned P1 SessionCgroup usage/PSI is merged with fresh machine headroom, displayed by ResourceView and forwarded to real inference admission. Typed high-PSI regression plus live coordinator/cgroup fixture; missing owned boundary fails closed. Team review stays P3-010. |
| P3-095 | Cross-harness routing experiments | Implemented | Actual OpenCode/Hermes parent-child copy workflow succeeds in separate worktrees with one authenticated inference queue and independently verified artifact hashes. Failed bridge/approval trials retained; no general coding success-rate claim. |
| P3-096 | Supervised A2A gateway | Implemented | Explicit loopback A2A 0.3 HTTP subset with Agent Card, private sponsor-bound credential, text drafts/get/cancel and immutable message dedup. Actual auth/scope/substitution tests; no automatic execution/full conformance claim. |
| P3-097 | Optional remote execution prototype | Implemented | Opt-in SSH relay with dedicated key, pinned host identity, explicit consent and disabled ambient auth/forwarding; actual rootless loopback sshd launch/replay/cancel cleanup test. No physical second-machine/partition rollback claim. |
| P3-098 | Fairness/responsiveness tests | Implemented | Real nested 1-core parent/two half-core child contention records near-equal CPU use, owner IPC latency and native Qt frame intervals; cleanup verified. Offscreen/bounded test, not full KWin input or whole-machine saturation qualification. |

## Phase 8 — Reliability, packaging and alpha

| ID | Task | Status | Evidence / remaining acceptance |
| --- | --- | --- | --- |
| P3-099 | Stabilize shared APIs/IPC | Partial | Content-addressed candidate, atomic server-side pin checks, actual historical Rust round trips and a committed protocol manifest checked by CI. Coordinated compatibility review and stable-release acceptance remain open. |
| P3-100 | Persistent metadata/recovery | Implemented | Durable registry, artifact snapshots, archive ledger and explicit unknown outcomes. |
| P3-101 | Shutdown/orphan cleanup | Implemented | Groups/scopes drained, startup cleanup, child cascade and crash tests. |
| P3-102 | Backend unavailable behavior | Implemented | Bounded actionable errors, terminal failure evidence and no silent provider fallback. |
| P3-103 | Health/diagnostics/errors | Implemented | Owner health, adapter status, host capability and supported diagnostics APIs. |
| P3-104 | Performance/memory-leak regressions | Implemented | Reproducible 100-cycle lifecycle/archive regression after 10 warmups: 471,040-byte RSS growth, 148.22 ms p95, explicit 32 MiB/5 s thresholds; bounded synthetic coverage, not a proof of no leaks. |
| P3-105 | Hermes/OpenCode/Codex/Claude mixed workload | Pending | Local harnesses exercised separately; authenticated cloud/mixed workflow not validated. |
| P3-106 | Five-agent local/cloud scenario | Pending | Requires dedicated authorized cloud/harness configuration and the actual reference run. Shared managed-domain reservations are implemented; unregistered backend processes and bypassing workloads remain outside their coverage. |
| P3-107 | Pin/test supported versions | Partial | Reproducible offline inventory verifies all installed harness/backend/engine pins; local adapter/protocol regressions and evidence matrix exist. Full authenticated cloud feature compatibility remains unqualified until deferred cloud tests. |
| P3-108 | Install/config/troubleshooting docs | Implemented | Current runtime guide and reproducible commands; historical weekly guide retained. |
| P3-109 | Person 1 image/startup integration | Implemented | Staged exact Rust/native Qt executables, desktop entry and user units; isolated runtime-linked service dependency startup, hardening, forced restart and session recovery verified. No ISO boot/upgrade/signing claim; distribution/release acceptance remains separate. |
| P3-110 | Person 4 demo/evaluation support | Implemented | Working native Qt observer, actual live API screenshot, bounded frame/IPC contention measurements and reproducible demo/benchmark scripts supplied for Person 4. No complete desktop-shell or physical-display evaluation claim. |
| P3-111 | Publish inference/GPU support matrices | Implemented | Explicit backend, protocol, hardware and enforcement support in research/runtime docs. |
| P3-112 | Release integration/regression review | Pending | Automated/local checks completed; cross-track release review and alpha acceptance remain outstanding. |

Outstanding work follows the acceptance conditions above. P3-087 is closed for
its technical resource-request design and tested registered-domain mechanism;
this is not a claim of Person 1 agreement or complete workstation accounting.
P3-010/P3-099/P3-112 still need actual cross-track review and release acceptance.
P3-085/P3-105/P3-106/P3-107 need the dedicated cloud configuration and live runs
deferred until last by the user. Owned model-server processes and administrative
coordinator grants outside managed agent roots are not automatically included in
the new authority. Hardware GPU partitioning and unsupported cache engines retain
their explicit limitations. The weekly plan remains unchanged.
