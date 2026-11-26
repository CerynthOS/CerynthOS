#!/usr/bin/env python3
"""Exercise token-authenticated shared inference against an existing local Ollama."""
import argparse
import concurrent.futures
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import uuid
import sys
import selectors


def rpc(directory, admin, operation):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(190)
        stream.connect(str(directory / ("control.sock" if admin else "inference.sock")))
        stream.sendall(json.dumps({"version": 1, "operation": operation}).encode() + b"\n")
        data = stream.makefile("rb").readline(4 * 1024 * 1024 + 1)
    response = json.loads(data)
    if "Err" in response["result"]:
        raise RuntimeError(response["result"]["Err"])
    return response["result"]["Ok"]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/cerynth-inferenced")
    parser.add_argument("--endpoint", default="http://127.0.0.1:11434")
    parser.add_argument("--model", default="qwen3:0.6b")
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--output", default="artifacts/person3/inference-smoke.json")
    parser.add_argument("--cuda-contention", action="store_true", help="Run a bounded 64 MiB CUDA bandwidth fixture during requests")
    args = parser.parse_args()
    assert 1 <= args.trials <= 20
    with tempfile.TemporaryDirectory(prefix="cerynth-inference-smoke-") as directory:
        directory = Path(directory)
        daemon = subprocess.Popen([str(Path(args.binary).resolve()), "--directory", str(directory), "serve"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        contention = None
        contention_ready = None
        try:
            deadline = time.monotonic() + 5
            while not (directory / "control.sock").exists():
                if daemon.poll() is not None:
                    raise RuntimeError(daemon.stderr.read().decode())
                if time.monotonic() > deadline:
                    raise TimeoutError("inference daemon did not start")
                time.sleep(0.02)
            rpc(directory, True, {"type": "configure", "endpoint": {"kind": "ollama", "id": "local", "url": args.endpoint}})
            grants = []
            for _ in range(2):
                session = str(uuid.uuid4())
                grants.append(rpc(directory, True, {"type": "register", "grant": {
                    "session_id": session, "endpoint_id": "local", "trust_domain": "smoke-public-fixture",
                    "allowed_models": [args.model], "max_priority": 10,
                    "concurrency_limits": [[f"session:{session}", 1], ["project:smoke", 1]]}}))
            def request(grant):
                return rpc(directory, False, {"session_id": grant["session_id"], "token": grant["token"],
                    "model": args.model, "prompt": "Reply with only the number: what is 2 plus 2?",
                    "max_output_tokens": 32, "priority": 0})
            if args.cuda_contention:
                contention = subprocess.Popen([sys.executable, str(Path(__file__).with_name("cuda_contention.py")),
                    "--seconds", "120"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                with selectors.DefaultSelector() as selector:
                    selector.register(contention.stdout, selectors.EVENT_READ)
                    if not selector.select(timeout=10):
                        raise TimeoutError("CUDA fixture did not become ready")
                line = contention.stdout.readline()
                if not line:
                    raise RuntimeError(contention.stderr.read())
                contention_ready = json.loads(line)
            trials = []
            for trial in range(args.trials):
                start = time.monotonic()
                with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                    results = list(pool.map(request, grants))
                trials.append({"trial": trial, "wall_seconds": time.monotonic() - start,
                    "requests": [{"session_id": r["session_id"], "metrics": r["metrics"],
                                  "response_nonempty": bool(r["response"]["message"]["content"].strip())} for r in results]})
            metrics = rpc(directory, True, {"type": "metrics"})
            models = rpc(directory, True, {"type": "models", "endpoint_id": "local"})
            details = rpc(directory, True, {"type": "details", "endpoint_id": "local", "model": args.model})
            rpc(directory, True, {"type": "revoke", "session_id": grants[0]["session_id"]})
            rejected = False
            try:
                request(grants[0])
            except RuntimeError:
                rejected = True
            assert rejected
            assert all(req["response_nonempty"] for trial in trials for req in trial["requests"])
            report = {"kind": "real-local-ollama-smoke", "model": args.model, "trials": trials,
                "cuda_contention": contention_ready,
                "metrics": metrics, "loaded_models": models["running"],
                "model_details": details.get("details"), "capabilities": details.get("capabilities"),
                "revoked_token_rejected": rejected, "cloud_requests": 0,
                "limitations": ["Synthetic prompt; not a coding-agent completion benchmark",
                                "Single shared backend; separate-model comparison is not measured here"]}
            output = Path(args.output)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps({"model": args.model, "trials": len(trials), "p50_ms": metrics["p50_ms"],
                              "p95_ms": metrics["p95_ms"], "revocation_verified": rejected, "report": str(output)}, indent=2))
        finally:
            if contention is not None:
                contention.terminate()
                try:
                    contention.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    contention.kill()
                    contention.wait()
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait()


if __name__ == "__main__":
    main()
