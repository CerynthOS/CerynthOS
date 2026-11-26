#!/usr/bin/env python3
"""Opt-in single-host comparison of one shared Ollama engine and two engines.

Owns every backend it starts. Does not download models, use cloud providers,
terminate other model servers, or claim statistical generality from three trials.
"""
import argparse
import concurrent.futures
import json
from pathlib import Path
import platform
import socket
import statistics
import subprocess
import tempfile
import time
import uuid

from inference_runtime_smoke import rpc


def free_port():
    with socket.socket() as stream:
        stream.bind(("127.0.0.1", 0))
        return stream.getsockname()[1]


def gpu_memory():
    result = subprocess.run(["nvidia-smi", "--query-gpu=uuid,memory.total,memory.used",
                             "--format=csv,noheader,nounits"], capture_output=True, text=True, timeout=5)
    result.check_returncode()
    return [{"uuid": fields[0].strip(), "total_mib": int(fields[1]), "used_mib": int(fields[2])}
            for line in result.stdout.splitlines() if (fields := line.split(","))]


def scenario(args, shared):
    with tempfile.TemporaryDirectory(prefix="cerynth-model-comparison-") as directory:
        directory = Path(directory)
        daemon = subprocess.Popen([str(Path(args.binary).resolve()), "--directory", str(directory),
                                   "serve", "--concurrency", "2"], stdout=subprocess.DEVNULL,
                                  stderr=subprocess.PIPE)
        owned = []
        try:
            deadline = time.monotonic() + 5
            while not (directory / "control.sock").exists():
                if daemon.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("coordinator startup failed")
                time.sleep(0.02)
            before = gpu_memory()
            assert before and before[0]["total_mib"] - before[0]["used_mib"] >= 3072, "Need 3 GiB free VRAM including desktop margin"
            for index in range(1 if shared else 2):
                endpoint_id = f"engine-{index}"
                rpc(directory, True, {"type": "start_backend", "spec": {
                    "id": endpoint_id, "binary": str(Path(args.ollama).resolve()),
                    "models_directory": str(Path(args.models_directory).resolve()),
                    "endpoint": f"http://127.0.0.1:{free_port()}", "context_length": 4096,
                    "parallel_requests": 2 if shared else 1}})
                owned.append(endpoint_id)
            grants = []
            for index in range(2):
                session_id = str(uuid.uuid4())
                grants.append(rpc(directory, True, {"type": "register", "grant": {
                    "session_id": session_id, "endpoint_id": owned[0 if shared else index],
                    "trust_domain": "public-benchmark-fixture", "allowed_models": [args.model],
                    "concurrency_limits": [[f"session:{session_id}", 1]], "max_priority": 0}}))

            def infer(grant):
                result = rpc(directory, False, {"session_id": grant["session_id"], "token": grant["token"],
                    "model": args.model, "prompt": "Write a short explanation of why 2 plus 2 equals 4.",
                    "max_output_tokens": 64, "priority": 0})
                response = result["response"]
                return {"metrics": result["metrics"], "nonempty": bool(response["message"]["content"]),
                        "prompt_tokens": response.get("prompt_eval_count"), "output_tokens": response.get("eval_count"),
                        "engine_total_ns": response.get("total_duration")}

            # Cold starts are reported separately from warm concurrency trials.
            cold = [infer(grant) for grant in (grants[:1] if shared else grants)]
            warmup = [infer(grant) for grant in grants]
            loaded = {endpoint: rpc(directory, True, {"type": "models", "endpoint_id": endpoint})["running"]
                      for endpoint in owned}
            memory = gpu_memory()
            trials = []
            for index in range(args.trials):
                start = time.monotonic()
                with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                    results = list(pool.map(infer, grants))
                trials.append({"trial": index, "wall_seconds": time.monotonic() - start, "requests": results})
            assert all(result["nonempty"] for trial in trials for result in trial["requests"])
            return {"shared": shared, "parallelism_per_engine": 2 if shared else 1,
                    "cold": cold, "warmup": warmup, "trials": trials, "loaded_models": loaded,
                    "gpu_before": before, "gpu_loaded": memory,
                    "mean_pair_seconds": statistics.mean(trial["wall_seconds"] for trial in trials)}
        finally:
            for endpoint in owned:
                try:
                    rpc(directory, True, {"type": "stop_backend", "endpoint_id": endpoint})
                except (OSError, RuntimeError):
                    pass
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/cerynth-inferenced")
    parser.add_argument("--ollama", default="/usr/local/bin/ollama")
    parser.add_argument("--models-directory", required=True)
    parser.add_argument("--model", default="qwen3:0.6b")
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--output", default="artifacts/person3/shared-model-benchmark.json")
    args = parser.parse_args()
    assert 3 <= args.trials <= 20
    report = {"kind": "single-host-synthetic-shared-vs-separate", "model": args.model,
              "kernel": platform.release(), "ollama": subprocess.check_output([args.ollama, "--version"], text=True).strip(),
              "cloud_requests": 0, "scenarios": [], "limitations": [
                  "One GPU, one small model and fixed scenario order; no broad speedup claim",
                  "GPU usage includes desktop and other workloads; readings are snapshots, not isolated peaks",
                  "Cold load and warm request timings are separate; backend prompt reuse can affect results"]}
    for shared in [True, False]:
        report["scenarios"].append(scenario(args, shared))
        print(json.dumps({"shared": shared, "mean_pair_seconds": report["scenarios"][-1]["mean_pair_seconds"]}), flush=True)
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
