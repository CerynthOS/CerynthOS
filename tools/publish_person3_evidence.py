#!/usr/bin/env python3
"""Export selected public-fixture evidence, excluding prompts/output and runtime identities."""
import hashlib
import json
from pathlib import Path
import re
import statistics

ROOT=Path(__file__).resolve().parents[1]
SOURCE=ROOT/"artifacts/person3"
DESTINATION=ROOT/"docs/benchmarks/person3"
PRIVATE_KEYS={"session_id","parent_id","source_session_id","checker_session_id","id","profile_digest",
    "events","harness_events","synthetic_task_output","verification","artifact","metrics","device_id"}


def clean(value,keep_metrics=False):
    if isinstance(value,dict):return {key:clean(item,keep_metrics) for key,item in value.items() if key not in PRIVATE_KEYS or (key=="metrics" and keep_metrics)}
    if isinstance(value,list):return [clean(item,keep_metrics) for item in value]
    if isinstance(value,str):return value.replace(str(ROOT),"<repo>").replace(str(Path.home()),"<home>")
    return value


def publish(name,keep_metrics=False):
    source=SOURCE/name
    data=json.loads(source.read_text())
    result=clean(data,keep_metrics)
    destination=DESTINATION/name
    destination.write_text(json.dumps({"source_artifact":name,"source_sha256":hashlib.sha256(source.read_bytes()).hexdigest(),
        "selection":"Public synthetic metrics/booleans; raw prompts/output, runtime identities and credentials excluded",
        "result":result},indent=2)+"\n")
    return result


def main():
    DESTINATION.mkdir(parents=True,exist_ok=True)
    files=["engine-vllm-prefix-on-instrumented.json","engine-vllm-prefix-off-instrumented.json",
        "engine-sglang-prefix-on-linker.json","engine-sglang-prefix-off-linker.json","engine-lmcache-cpu-offload.json",
        "engine-vllm-coordinator-isolation.json","lmcache-import-probe.json","native-guard-runtime-smoke-final.json",
        "gpu-runtime-smoke-final.json","remote-runtime-smoke.json","placement-runtime-smoke.json",
        "a2a-runtime-smoke.json","task-evidence-runtime-smoke.json","cross-harness-runtime-smoke.json",
        "cross-harness-runtime-smoke-diagnostic.json","cross-harness-runtime-smoke-call-hint.json"]
    for name in files:publish(name)
    continuation=["resource-hierarchy-smoke.json","auth-inventory-smoke.json","observer-runtime-smoke.json",
        "resource-responsiveness-smoke.json","model-residency-smoke.json","mps-capability-probe.json",
        "packaging-runtime-smoke.json","runtime-version-inventory.json","cross-harness-terminal-profile.json",
        "cross-harness-terminal-profile-final.json","cross-harness-terminal-profile-accepted.json","cross-harness-terminal-copy.json"]
    for name in continuation:publish(name,keep_metrics=True)
    experiments=[]
    for name in files[:5]:
        data=json.loads((SOURCE/name).read_text());groups=[]
        for prefix in sorted({trial["prefix_words"] for trial in data["trials"]}):
            trials=[trial for trial in data["trials"] if trial["prefix_words"]==prefix]
            describe=lambda values:{"n":len(values),"mean":statistics.mean(values),
                "sample_stddev":statistics.stdev(values) if len(values)>1 else None,"min":min(values),"max":max(values)}
            samples=[sample for trial in trials for sample in [trial["fresh"],trial["repeat"]]]
            groups.append({"prefix_words":prefix,"fresh_ttft_ms":describe([t["fresh"]["ttft_ms"] for t in trials]),
                "repeat_ttft_ms":describe([t["repeat"]["ttft_ms"] for t in trials]),
                "repeat_minus_fresh_request_ms":describe([t["repeat"]["request_ms"]-t["fresh"]["request_ms"] for t in trials]),
                "completion_tokens_per_sum_request_second":sum(s["usage"]["completion_tokens"] for s in samples)/sum(s["request_ms"]/1000 for s in samples)})
        experiments.append({"source":name,"engine":data["engine"],"native_prefix_cache":data["cache"],
            "lmcache_cpu_offload":data.get("lmcache_cpu_offload",False),"gpu_memory_ready_mib":data["gpu_memory_ready_mib"],"groups":groups})
    failures=[]
    for name in ["engine-vllm-prefix-on.json","engine-vllm-prefix-on-toolkit.json","engine-sglang-prefix-on.json","engine-sglang-prefix-on-toolkit.json"]:
        data=json.loads((SOURCE/name).read_text());failures.append({"source":name,"source_sha256":hashlib.sha256((SOURCE/name).read_bytes()).hexdigest(),
            "success":data["success"],"error_type":data.get("error_type"),"cleanup_scope_stopped":data["cleanup_scope_stopped"]})
    transfers=[]
    text=(SOURCE/"engine-lmcache-cpu-offload.backend.log").read_text(errors="replace")
    for line in text.splitlines():
        match=re.search(r"(Stored|Retrieved) (\d+) out of.*?size: ([\d.]+) [gG][bB], cost ([\d.]+) ms, throughput: ([\d.]+) GB/s",line)
        if match:transfers.append({"operation":match[1].lower(),"tokens":int(match[2]),"reported_size_gb":float(match[3]),
            "cost_ms":float(match[4]),"reported_gb_per_second":float(match[5])})
    (DESTINATION/"engine-cache-comparison.json").write_text(json.dumps({"kind":"single-host-engine-cache-comparison",
        "model":"Qwen/Qwen3-0.6B","model_revision":"c1899de289a04d12100db370d81485cdf75e47ca","trials_per_prefix":3,
        "experiments":experiments,"failed_startups":failures,"lmcache_reported_transfers":transfers,
        "limitations":["One host; very short two-token outputs; fixed engine order; sequential fresh/repeat pairs",
            "Observed completion rate includes prefill/request latency; not long-generation throughput",
            "Memory observations include desktop/driver allocations; no isolated memory causal claim",
            "LMCache wheel native extension ABI mismatch; connector used available fallback paths",
            "No Cerynth cache recommendation was actuated; no claim of a Cerynth policy speedup"]},indent=2)+"\n")
    trials=[]
    for name in ["opencode-managed-http-smoke.json","opencode-managed-http-worktree-smoke.json",
        "opencode-managed-acp-smoke.json","opencode-managed-acp-authenticated-smoke.json",
        "opencode-native-guard-coding-smoke.json","opencode-native-guard-qwen3-4b-smoke.json",
        "opencode-native-guard-spaced-fixture.json","opencode-native-guard-explicit-task.json"]:
        data=json.loads((SOURCE/name).read_text())
        trials.append({"source":name,"source_sha256":hashlib.sha256((SOURCE/name).read_bytes()).hexdigest(),
            **{key:data.get(key) for key in ["harness","model","transport","native_guard","fixture_style","task_prompt_sha256",
                "state","exit_code","worktree","boundary","coding_task_verified","network_policy"]},
            "independent_checker_boundary":data["verification"]["boundary"],
            "normalized_tool_events":sum(e["kind"].startswith("harness_tool") for e in data["harness_events"])})
    (DESTINATION/"additional-harness-trials.json").write_text(json.dumps({"kind":"additional-exploratory-harness-trials",
        "trials":trials,"controlled_success_rate_claim":False,"cloud_requests":0},indent=2)+"\n")
    print(json.dumps({"exported_sources":len(files)+len(continuation),"additional_harness_trials":len(trials),"lmcache_transfer_samples":len(transfers)}))


if __name__=="__main__":main()
