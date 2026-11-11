# CacheWeaver backend study

Researched 2026-11-10. vLLM 0.25.0 and LMCache 0.3.7 documentation are versioned
references below. SGLang and vLLM's newer offload guide are rolling documentation;
their exact installed release must be pinned before a live integration. None of
these three engines has been benchmarked by Cerynth on this host.

## Reuse the engine's cache machinery

vLLM's prefix cache identifies full token blocks using the prefix chain and
additional context such as adapters, multimodal inputs and tenant salt. Active
references and the free/eviction queue belong to the engine. Cerynth should
coordinate requests and budgets rather than editing these structures. A common
model name or similar text does not establish an identical reusable token prefix.
An owner-derived `cache_salt` can separate reuse groups; agents must not choose a
different group's salt. Cerynth currently rejects client-supplied salts and uses
separate backend origins for different trust domains; it does not claim verified
engine-level multi-tenant cache isolation.
[vLLM 0.25.0 prefix design](https://docs.vllm.ai/en/v0.25.0/design/prefix_caching/).

The newer vLLM offload connector supports a CPU tier and optional additional
storage tiers. Per-request loading/offloading token caps are experimental and
version-sensitive. Storage transfers, pinned RAM and I/O contention can cost
more than recomputation. Cerynth must obtain real completion/failure evidence
from a supported connector before claiming a transfer occurred. Native transfer
addresses and cache controls are rejected on the generic inference proxy until
a trusted backend-specific adapter owns them.
[vLLM offload guide](https://docs.vllm.ai/en/latest/features/kv_offloading_usage/).

SGLang's HiCache separates GPU, host and storage tiers. Host cache belongs to an
inference instance; it is not a pooled multi-machine RAM cache. Cross-instance
reuse depends on the configured storage tier and namespace. That namespace is a
privacy boundary requiring owner-authorized storage and credentials.
[HiCache design](https://docs.sglang.io/docs/advanced_features/hicache_design).

SGLang already offers session-aware references through UnifiedRadixCache.
References influence eviction but do not pin memory; closing a session removes
references rather than guaranteeing immediate release. A future adapter must
derive session IDs from authenticated Cerynth identity, pass the full intended
prompt on each turn, and close references on completion, error and cancellation.
These native facilities should be evaluated before inventing equivalent cache
ownership in Cerynth.
[Session-aware cache](https://docs.sglang.io/docs/advanced_features/session_radix_cache).

SGLang also supplies several eviction policies, including recency, frequency,
request priority and a tail-latency policy. Evictable-leaf eligibility remains an
engine decision. Priority inputs therefore need to come from owner grants, not
untrusted request fields. Our experimental recommendation policy must be compared
against the native policy that best fits the workload, as well as default LRU.
[Eviction policies](https://docs.sglang.io/docs/advanced_features/radix_eviction_policy).

LMCache connects inference engines to cached KV storage. Lookup/retrieval occurs
before affected computation, while storage writes may continue after the user
response. A finished inference request consequently does not prove that cache
storage is durable or background work has stopped. Integration needs explicit
backend capabilities, storage completion evidence and namespace-specific access
controls. Published upstream speedup figures are not Cerynth measurements.
[LMCache 0.3.7 integration](https://docs.lmcache.ai/v0.3.7/developer_guide/integration.html).

## Current implementation and measurements

`cerynth-model-api::cache` makes deterministic metadata-based recommendations:
retain active/interactive data, consider only the same trust domain, and compare
transfer costs with reload/recomputation estimates. Offload is recommended only
when explicitly supported. It does not operate a tensor store or alter an
Ollama/vLLM/SGLang cache. Policy tests do not establish a latency improvement.

The normalized model registry separates downloaded weight bytes, observed
allocated memory/VRAM, architecture context capacity and configured active
context. Residency observations become unknown after restart. Native harness
events retain optional reported cached-input/cache-write token counts separately
from ordinary input/output usage. Missing counters are unknown, not zero.

`tools/cache_baseline.py` starts an owned Ollama backend, records a cold request,
then three fresh/repeated prompt pairs at each of three prefix sizes, followed
by repeated switches between two installed models. Reports preserve engine load,
prefill and generation durations plus actual prompt-token counters. Fresh prompts
have distinct leading fixture IDs. No model is downloaded and no other backend
is stopped or unloaded. A repeated-prompt timing difference is not by itself a
verified cache-hit measurement.

## Controlled engine evaluation still required

For each supported engine/version, use identical model artifacts, tokenizer,
dtype/quantization, context capacity, device and concurrency. Test short and long
prefixes, branch divergence, multiple active sessions and pressure from a bounded
GPU job. Separate process/driver startup, weight loading, prefill, decode and
coordinator queue latency. Retain failures and raw per-trial counters.

Compare engine defaults, a suitable native alternative, and one session-aware
policy under the same memory budget. Record TTFT and total-latency distributions,
reused tokens, transfer bytes/times, host pressure, storage I/O and verified task
completion. Include randomized trial order, warmup rules and enough repetitions
for tail estimates. The small smoke experiments in this branch do not meet that
research acceptance gate.

For privacy, test identical private prefixes in different authorization domains,
including endpoint aliases, restart, revoked sessions and shared storage
namespaces. An ordinary speed benchmark cannot establish absence of cache leaks.
An engine adapter may use native cache isolation only after demonstrating its
actual enforcement behavior; URL separation is conservative routing policy,
not proof that a remote operator uses independent engines.
