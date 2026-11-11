# Person 3 runtime validation

Date: November 11, 2026. Branch: `feat/person3-agent-runtime`. Upstream main:
`6a6f0183216eb609e2d0f5acda89b0ec61a41a64`. Measurements were collected on
November 10–11. This is a development validation report, not public-alpha or
cross-track acceptance. The weekly plan is unchanged. Additional integrations,
engine experiments and failed task outcomes are detailed in the
[continuation report](2026-11-11-person3-continuation.md).

## Implemented runtime

Eight new Rust packages provide the versioned agent API, session supervisor,
generic PTY transport, native CLI adapters, protocol clients, model API,
inference coordinator and cooperative resource broker. The supervisor owns
durable operation IDs, process/scope cleanup, bounded event/output retention,
delegation, isolated worktrees and immutable artifact snapshots. The separate
coordinator provides endpoint/model registration, authenticated session grants,
streaming proxy routes and hierarchical inference admission.

The ordinary launch path uses Bubblewrap plus systemd. The optional Guard path
integrates Person 2's actual branch revision
`e592fa47797179ad24a99fc76143be48191a7649` for offline generic commands. Its
source was built in an ignored detached worktree, without changing or merging
that branch. A subsequent opt-in OpenCode root gateway now binds native model
traffic to a private per-root grant; see the continuation for exact limits.

## Automated and actual-host checks

| Check | Result | Evidence |
| --- | --- | --- |
| `cargo test --locked --workspace` | 228 passed, 0 failed, 3 upstream doctest examples ignored | [Automated result](../benchmarks/person3/automated-validation.json) |
| Strict Clippy, all targets of all eight new packages | Passed with warnings denied | Same automated result; upstream workspace manifest warning remains outside new-package code |
| Format, generated schemas, shell/Python syntax and staging | Passed; both user units pass `systemd-analyze --user verify` with staged executable paths | Same automated result; no live service installation |
| Managed namespace/scope and SIGKILL recovery | Host fixture hidden, network denied, effective limits read back; restart empties scope and preserves unknown operation outcome | [Runtime result](../benchmarks/person3/runtime-parent-guard-smoke.json) |
| Git worktree with hostile hooks/filter | Hooks/filter disabled, source config unchanged, separate checkout starts as managed session | [Worktree result](../benchmarks/person3/worktree-runtime-smoke.json) |
| Pinned real Guard integration | Seccomp active; host/network/approval access denied; child escalation denied; leaf-first cleanup; unavailable Guard never falls back | [Guard result](../benchmarks/person3/guard-parent-guard-smoke.json) |
| Native OpenCode HTTP discovery | Password authentication enforced, anonymous request rejected, native metadata found, no registry import/control acquired | [API result](../benchmarks/person3/opencode-api-smoke.json) |
| Model registry/discovery | Reachable and unreachable explicit loopback candidates distinguished; registration remains owner-controlled | [Model result](../benchmarks/person3/model-registry-smoke.json) |
| 100 managed lifecycle/archive cycles after 10 warmups | RSS growth 471,040 bytes; p95 lifecycle 148.22 ms; below configured 32 MiB/5 s regression thresholds | [All samples](../benchmarks/person3/agent-runtime-stress.json) |

The stress result covers this bounded synthetic workload, not proof of no leaks
over arbitrary durations. Unit/integration tests include private IPC, queue
cancellation/revocation, replay/fingerprint protection, corrupt-state rejection,
parent bounds, tool-error/cache-counter normalization, symlink-safe artifacts
and structured protocol negotiation. Tests using synthetic backends do not
establish live cloud/provider or driver compatibility.

The tested compiler is Rust 1.99.0. The workspace declares Rust 1.85, but its
existing upstream dependency set already requires newer compilers; this work
does not establish a passing 1.85 build. Existing locked dependency versions
were retained; new transitive version variants were added for the new packages.
CI now checks strict linting of the eight packages and generated-schema drift
alongside the existing full-workspace checks.

## Coding task outcomes

OpenCode 1.19.5 and Ollama 0.35.0 ran a public synthetic task: fix a subtraction
implementation of `add`, then verify two cases. Two final reruns with
`qwen2.5:3b` yielded one verified success and one failure despite both harnesses
exiting zero. The failure wrote a literal escaped newline and did not execute a
meaningful test. The independent checker caught it. Both final checkers ran in
fresh managed Bubblewrap/systemd sessions with networking denied.

Earlier exploratory OpenCode, Hermes and Codex trials used a host-side checker;
that unsafe verification arrangement was corrected. Those older trials remain
labelled **legacy-host-side-checker**, and are not evidence for the new checker
boundary. Hermes 0.23.1 and Codex 0.167.0 launch/inference paths were exercised,
but the tested small models did not produce independently verified coding fixes.
Claude Code 2.1.312 was detected; authenticated cloud execution was not tested.

[All 15 retained harness trial summaries](../benchmarks/person3/harness-trials.json)
include failures, checker method, source artifact hashes, reported cache counters
and available before/after model observations. They span changing models and
configuration, so their aggregate is not a controlled success-rate estimate.
The two fixed cases are synthetic functional checks, not general adversarial
verification of model-generated programs. No cloud task context was transmitted.

The final OpenCode observations record `qwen2.5:3b` loaded at a configured
16,384-token context with 2,721,946,009 allocated GPU bytes. Cache-read token
counts come from native harness reports, not an independently verified GPU
cache hit counter. Residency and usage are correlated by session without
claiming backend cache isolation.

## Shared versus separate model experiment

The host has 24 logical CPUs, about 16.45 GB RAM, and an RTX 5060 Laptop GPU
with 8,151 MiB VRAM. [Hardware observations](../benchmarks/person3/hardware.json)
record driver and unavailable Intel GPU metrics. One shared Ollama engine with
parallelism two was compared with two separate engines at parallelism one,
using `qwen3:0.6b`, identical synthetic prompts and three warm request-pair trials.

| Setup | Warm pair wall times (seconds) | Mean (seconds) | Engine allocated model bytes |
| --- | --- | --- | --- |
| Shared | 0.3055, 0.2659, 0.2416 | 0.2710 | 1,431,631,298 |
| Separate | 0.5212, 0.4207, 0.4985 | 0.4801 | 2 × 930,401,484 |

[Raw numerical trials](../benchmarks/person3/shared-model-benchmark.json) retain
cold/warmup results and GPU observations. The shared cold request took 57.2 s;
the separate cold requests took about 2.37/2.23 s. This outlier is retained.
Fixed ordering, three trials, differing engine parallelism, shared desktop load
and one small model prevent a general causal performance claim. These numbers
justify further controlled experiments, not a system-wide speedup assertion.

## Context and accelerator baselines

The [cache/loading baseline](../benchmarks/person3/cache-baseline.json) uses
three fresh/repeated prompt pairs at 128, 512 and 2,048 prefix words with a
16,384-token context. The first cold request took 3.908 s, including 3.481 s
reported loading. Fresh/repeated TTFT was approximately 40–78/14–23 ms,
94–98/17–30 ms and 261–343/42–52 ms respectively. Engine prompt-evaluation
counts remained the full prompt length in both conditions; they are not verified
cache-hit counts. Six alternating model switches retain their load timings,
including a 19.44 s outlier. No Cerynth cache policy was applied to the engine.

A bounded Python/ctypes CUDA bandwidth job allocated 64 MiB while real local
inference ran. [Warm baseline](../benchmarks/person3/inference-baseline-fully-warm.json)
and [contention trials](../benchmarks/person3/inference-cuda-contention.json)
demonstrate simultaneous activity and queue metrics. Their short prompts,
fixed order and six requests per condition are insufficient for a useful
contention-performance conclusion. No GPU partition, preemption or VRAM
enforcement is claimed. MIG is unavailable on this host; MPS was not configured.

The [engine study](../research/cacheweaver-backends.md) and
[continuation](2026-11-11-person3-continuation.md) now include actual vLLM/SGLang
native cache and LMCache CPU transfer experiments, with failures and compatibility
limits retained. The Cerynth metadata policy still does not actuate engine KV
transfers; no Cerynth policy performance gain has been demonstrated.

## Remaining acceptance

The [112-task ledger](../roadmap/person3-tasks.md) is the status source. Outstanding
work includes Person 1's resource/pressure interface and nested kernel accounting,
authoritative arbitrary-tool effect deduplication, model-residency actuation,
broader cache/privacy validation, consented mixed cloud workflows, the desktop
consumer, and image/release integration. The local prototypes do not substitute
for team agreement or release review.

Reproduction commands and prerequisites are in the
[runtime guide](../runtime/agent-runtime.md). Packaging stages binaries and user
units into an explicit directory; it does not enable services or install into
the running system. Private raw artifacts remain in ignored `artifacts/person3`;
the committed-ready evidence contains selected public-fixture measurements,
source hashes and no prompts, credentials, GPU UUIDs or process IDs.
