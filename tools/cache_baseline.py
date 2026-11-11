#!/usr/bin/env python3
"""Measure cold loading, repeated prefixes and model switching on an owned backend.

Uses only preinstalled models and public synthetic prompts. Engine counters are
reported as observed; a repeated prompt is not assumed to prove a KV-cache hit.
"""
import argparse
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import uuid

from inference_runtime_smoke import rpc


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/cerynth-inferenced")
    parser.add_argument("--ollama", default="/usr/local/bin/ollama")
    parser.add_argument("--models-directory", required=True)
    parser.add_argument("--model", default="qwen3:0.6b")
    parser.add_argument("--switch-model", default="qwen2.5:3b")
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--output", default="artifacts/person3/cache-baseline.json")
    args = parser.parse_args()
    assert 3 <= args.trials <= 20
    with socket.socket() as reserved:
        reserved.bind(("127.0.0.1", 0))
        port = reserved.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="cerynth-cache-baseline-") as root:
        directory = Path(root)
        daemon = subprocess.Popen([str(Path(args.binary).resolve()), "--directory", root, "serve"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        owned = False
        try:
            deadline = time.monotonic() + 5
            while not (directory / "control.sock").exists():
                if daemon.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("coordinator startup failed")
                time.sleep(0.02)
            rpc(directory, True, {"type": "start_backend", "spec": {
                "id": "benchmark", "binary": str(Path(args.ollama).resolve()),
                "models_directory": str(Path(args.models_directory).resolve()),
                "endpoint": f"http://127.0.0.1:{port}", "context_length": 16384,
                "parallel_requests": 1}})
            owned = True
            installed = rpc(directory, True, {"type": "models", "endpoint_id": "benchmark"})["installed"]
            names = {model["name"] for model in installed}
            assert {args.model, args.switch_model} <= names, "Install selected models explicitly before benchmarking"
            session_id = str(uuid.uuid4())
            grant = rpc(directory, True, {"type": "register", "grant": {
                "session_id": session_id, "endpoint_id": "benchmark", "trust_domain": "public-cache-fixture",
                "allowed_models": sorted({args.model, args.switch_model}), "max_priority": 0,
                "concurrency_limits": [[f"session:{session_id}", 1]]}})

            def infer(model, prompt):
                result = rpc(directory, False, {"session_id": session_id, "token": grant["token"],
                    "model": model, "prompt": prompt, "max_output_tokens": 16, "priority": 0})
                counters = {key: result["response"].get(key) for key in (
                    "prompt_eval_count", "prompt_eval_duration", "load_duration", "eval_count", "eval_duration", "total_duration")}
                return {"model": model, "metrics": result["metrics"], "engine_counters": counters,
                        "nonempty": bool(result["response"]["message"]["content"])}

            cold = infer(args.model, "Explain why 2 plus 2 equals 4.")
            prefixes = []
            for words in [128, 512, 2048]:
                for trial in range(args.trials):
                    prompt = f"Public fixture {uuid.uuid4()}: " + "context " * words + "\nReply only with the number 4."
                    fresh = infer(args.model, prompt)
                    repeated = infer(args.model, prompt)
                    prefixes.append({"prefix_words": words, "trial": trial, "fresh": fresh, "identical_repeat": repeated})
            switches = []
            for trial in range(args.trials):
                for model in [args.switch_model, args.model]:
                    switches.append({"trial": trial, **infer(model, "Reply only with the number 4.")})
            registry = rpc(directory, True, {"type": "registry", "endpoint_id": "benchmark", "model": args.model})
            report = {"kind": "single-host-cache-and-loading-baseline", "cloud_requests": 0,
                "backend": "ollama", "context_length": 16384, "parallel_requests": 1,
                "installed_models": [model for model in installed if model["name"] in {args.model, args.switch_model}],
                "cold_first_request": cold, "prefix_trials": prefixes, "switch_trials": switches,
                "registry": registry, "limitations": [
                    "Engine-reported prompt counters are not a verified cache hit counter",
                    "Identical-prefix trials always follow fresh-prefix trials; no randomized policy comparison",
                    "Loading includes backend/driver effects; one laptop does not establish general performance",
                    "No vLLM, SGLang or LMCache engine is exercised by this baseline"]}
            output = Path(args.output)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps({"prefix_pairs": len(prefixes), "switches": len(switches), "report": str(output)}))
        finally:
            if owned:
                try:
                    rpc(directory, True, {"type": "stop_backend", "endpoint_id": "benchmark"})
                except (OSError, RuntimeError):
                    pass
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait()


if __name__ == "__main__":
    main()
