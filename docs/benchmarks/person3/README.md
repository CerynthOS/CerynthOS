# Person 3 measured evidence

These JSON files preserve public synthetic measurements from November 10–19,
2026. The [validation report](../../integration/2026-11-11-person3-runtime.md)
describes procedures, failures and limits. This is one development host, not
research-grade evidence of general OS or cache-policy performance.

Numerical experiment files retain cold/warmup samples, all measured trials and
reported engine counters. Wrappers identify the ignored source artifact and its
SHA-256. Runtime smoke copies omit transient process/session/cgroup identity.
Hardware omits device UUIDs and process records. Harness summaries omit raw
prompts/output, retain failures and identify the verification method. Token
counts are usage measurements, never credential values.

Reproduce with the scripts under `tools/` and the pinned versions/prerequisites
in the [runtime guide](../../runtime/agent-runtime.md). Fresh results will vary;
preserve failures and use new artifact names when changing conditions. Sanitized
summaries are intentionally smaller than private raw artifacts, and do not
establish full trace/audit completeness.

The continuation adds pinned vLLM/SGLang native-cache comparisons, real LMCache
CPU transfers with an ABI/fallback limitation, actual coordinator salt checks,
native Guard identity, supervised CUDA, evidence collection, A2A and SSH results.
`additional-harness-trials.json` and the cross-harness files retain failed task
outcomes separately from successful transport/boundary checks. Re-export these
selected public fixtures with `python3 tools/publish_person3_evidence.py` after
running the corresponding tools; ignored raw backend logs are never published.

The [November 12 audit](../../integration/2026-11-12-person3-local-completion.md)
adds three-level systemd accounting/recovery, actual NVIDIA MPS client controls,
private native auth inventories, live Qt API consumption and frame observations,
packaged user-service restart, offline version pins, and successful independently
checked OpenCode/Hermes copy routing. Failed terminal-profile trials remain beside
the accepted copy fixture. `model-residency-smoke.json` compares an actuated
idle-model eviction recommendation with engine defaults and retains the observed
reload-latency regression. These fixtures make no cloud model requests.

The [November 19 audit](../../integration/2026-11-19-person3-remaining-partials.md)
adds the actual pinned Person 1 budget/usage/PSI integration, sandbox-mounted
Guard tool-effect deduplication with one independently counted local Git push,
owned Ollama session/cache-generation isolation and persisted privacy checks,
and live client contract pinning. Separate `*-nov19.json` files preserve Guard
and model-residency regressions without replacing the previous measured trials.
Unknown-acknowledgement handling is tested by the Rust fault fixture; no
exactly-once, arbitrary-shell, timing-noninterference or cloud result is implied.

The November 20 follow-up adds `shared-authority-smoke.json`, separate dated
contract/hierarchy/packaging results and `validation-summary-nov20.json`. These
cover two participating daemons and their registered root subtrees; they do not
assert accounting of unregistered model servers or bypassing clients. The
272-test result and raw-log hashes are retained separately from November 19.
