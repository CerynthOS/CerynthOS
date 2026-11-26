# Person 3 continuation: runtime integrations and engine experiments

Date: November 11, 2026. Development branch: `feat/person3-agent-runtime`.
The latest fetch still places upstream main at
`6a6f0183216eb609e2d0f5acda89b0ec61a41a64`. This extends the
[initial report](2026-11-11-person3-runtime.md); the weekly plan is unchanged.

## Working additions

| Area | Actual validation | Limits |
| --- | --- | --- |
| OpenCode HTTP | Managed password-protected owned server, create/prompt/abort/history; intermediate tool events; one independently verified public coding fixture | History events arrive after the prompt; no SSE subscription or native child mapping |
| OpenCode ACP | Real pinned ACP v1 session/prompt streaming and bounded synthetic client-operation denial tests | No host FS/terminal/MCP authority or native resume advertised; real coding fixtures failed |
| Native Guard inference | Actual pinned Guard, two private root gateways, credential/identity/model isolation, startup cancellation and SIGKILL recovery | OpenCode roots only, two gateways maximum, no native delegation/GPU profile; coding failures preserved |
| Supervised GPU jobs | Real 64 MiB CUDA workload in Guard/systemd, cooperative 256 MiB lease, immutable operation replay protection, timeout and cleanup | Guard decision label is advisory; agentd separately enforces launch admission; no hard VRAM/device partition |
| Evidence collection | Immutable child candidate/diff/output/test artifacts; separate managed checker; failed assertions, mutation and symlink cases | Exact owner-selected assertions only; no general correctness proof |
| Retries | Three owner-sponsored retry drafts, atomic predecessor links, restart deduplication; unknown/unclean outcomes denied | No automatic tool replay; owner must acknowledge prior effects |
| Placement | Real measured load samples, fresh device capacity and calibrated allocation; active/stale/unauthorized targets protected | Advisory only; no model eviction or migration |
| A2A HTTP | Real loopback Agent Card and authenticated sponsor-bound text task subset; deduplication/cancel/scope checks | Drafts require separate owner launch; no full A2A conformance or streaming |
| SSH prototype | Rootless loopback sshd, dedicated generated identity, strict pinned host key, remote managed execution/replay/cancellation | No physical second-machine or network-partition recovery claim |

Generic launch admission now also rejects a workspace that would expose a
separately configured inference/Guard control directory. A2A credentials must
remain within agentd's private control directory. These checks cover the actual
paths; they do not protect explicitly unisolated same-user processes.

The [acceptance ledger](../roadmap/person3-tasks.md) records **87 implemented,
20 partial and 5 pending** tasks under explicitly scoped acceptance. This is not
an assertion that the 112-task roadmap or public alpha is complete.

## Engine and cache experiments

Dedicated Python 3.12.14 environments ran PyTorch 2.11.0, vLLM 0.25.0,
SGLang 0.5.16 and LMCache 0.5.6. Exact Python package snapshots are under
[`docs/research/environments`](../research/environments). The offline model was
`Qwen/Qwen3-0.6B` revision `c1899de289a04d12100db370d81485cdf75e47ca`.
Downloads were explicit public artifact preparation; no provider credentials
were imported and no private task context was sent to cloud inference.

The fixture owns one bounded systemd scope at a time, reserves host RAM headroom,
uses eager execution and a 1,024-token context, and stops/verifies its own scope.
The servers bind authenticated numeric loopback. Models are local, remote model
code is disabled, and vLLM/LMCache usage telemetry is explicitly disabled. This research launcher is
resource-bounded; it is not labelled a sandbox.

Each condition retained nine fresh/repeat pairs: three trials at each of
64/192/320 prefix words, with fixed public prompt identifiers. All measured
answers were `4`. vLLM native-prefix enabled runs reported repeat hits of
96–368 tokens; disabled runs reported zero. SGLang enabled runs also reported
prefix hits. These are backend counters, rather than inferred timing improvements.

At 320 prefix words, the observed means were:

| Condition | Fresh TTFT ms | Repeat TTFT ms | Fresh total ms | Repeat total ms | GPU used at readiness MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| vLLM native prefix enabled | 22.79 | 25.21 | 30.64 | 33.39 | 1872 |
| vLLM native prefix disabled | 23.97 | 25.36 | 32.13 | 33.60 | 1872 |
| SGLang native radix enabled | 26.37 | 50.60 | 36.78 | 60.83 | 1791 |
| SGLang native radix disabled | 45.65 | 34.31 | 54.97 | 43.22 | 1791 |
| vLLM native prefix disabled + LMCache CPU | 59.34 | 38.69 | 67.67 | 47.72 | 1900 |

Every row has only three pairs at this size. The
[comparison dataset](../benchmarks/person3/engine-cache-comparison.json) includes
all conditions, dispersion and completion-token rate computed over request time.
Outputs contain only two generated tokens, so that rate is dominated by request
overhead and is not sustained decode throughput. Engine order was fixed, the
desktop remained active, and the engines used different KV allocation controls.
Compare each engine to its own configuration baseline; these numbers do not
establish a causal engine ranking or a Cerynth policy speedup. Cache reuse did
not consistently reduce latency, and LMCache added overhead on this small task.

LMCache used a bounded 256 MiB CPU tier and disabled vLLM native prefix reuse.
It stored and restored three 256-token chunks, about 0.0273 GB each. Reported
restore costs were 17.7868, 11.6844 and 12.1128 ms; the first store cost 64.1381 ms
and later stores about 6 ms. Its packaged native extension failed ABI import
against the pinned Torch. The external connector imported and executed using
available fallback paths. Retain that limitation when reproducing; dependency
resolution alone did not establish native binary compatibility.

Failed configurations are retained: missing nvcc, FlashInfer compiler/header
incompatibility, and SGLang's missing development linker alias. The successful
fixture uses native PyTorch sampling and a private libcudart linker alias,
without changing system libraries or disabling compiler compatibility checks.

## Cache authorization checks

A separate actual vLLM run used the Cerynth coordinator. Two grants sent the same
public prompt in A/A/B/B order. Counters showed miss/hit/miss/hit. The coordinator
derived opaque salts from the authenticated grant, rejected a caller-supplied
salt and rejected a revoked grant. Both its HTTP and Unix data paths also have
independent backend-body regression tests. Existing origin/domain restrictions
were retained.

This validates only the explicitly selected vLLM native-prefix chat path.
LMCache/external KV connectors, SGLang and broader timing/privacy attacks were
not validated for that mode. Do not infer their isolation from vLLM API compatibility.

## Agent completion and failures

[Additional harness trials](../benchmarks/person3/additional-harness-trials.json)
retain all eight new exploratory OpenCode HTTP/ACP/Guard results. One HTTP coding
trial passed the independent checker; the native Guard coding trials did not.
Observed failures include exact-edit mismatches, replacing a function with a
demonstration script, and a larger model reaching its deadline. Changing fixture
format or explicit instructions is recorded separately, not pooled as a success rate.

Three concurrent OpenCode/Hermes routing experiments used isolated worktrees,
the same authenticated local model, hierarchical grant scopes and global inference
concurrency one. Both adapters launched and emitted tool events. One diagnostic
OpenCode task produced the exact expected artifact; Hermes emitted an invalid
batch-tool-call structure or described a command without executing it. The
separate schema-hint condition also failed. These are failed heterogeneous task
acceptance results, with clean process/grant teardown. They do not establish a
reliable multi-agent workflow merely because both processes exited zero.

## Verification and outstanding work

The complete Rust workspace passed **228 tests, zero failures, three upstream
doctest examples ignored**. Strict Clippy passes all targets of the eight new
packages with warnings denied. Generated schemas, formatting, Python/shell
syntax, staging and user-unit verification are recorded in
[automated validation](../benchmarks/person3/automated-validation.json).

Outstanding engineering includes model-residency actuation, broader cache
privacy, global CPU/RAM reduced-grant decisions and reliable heterogeneous
completion. Person 1's resource/pressure API and nested accounting, Person 4's
consumer, shared API review, consented cloud workloads, physical remote testing
and distribution/release acceptance remain unperformed. No teammate agreement,
cloud result or release approval is fabricated. No services were installed or
enabled; owned experiment servers/scopes were stopped and model/environment
artifacts were retained for reproduction.
