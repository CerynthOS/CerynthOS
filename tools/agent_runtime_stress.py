#!/usr/bin/env python3
"""Opt-in managed-process regression: cleanup, latency, bounded retained memory.

Samples supervisor RSS after warmup, not the memory of an LLM or the whole OS.
Every session runs a synthetic true command and is archived after cleanup.
"""
import argparse
import json
import math
from pathlib import Path
import subprocess
import tempfile
import time
import uuid

from harness_runtime_smoke import rpc


def rss(pid):
    status = Path(f"/proc/{pid}/status").read_text()
    return next(int(line.split()[1]) * 1024 for line in status.splitlines() if line.startswith("VmRSS:"))


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/cerynth-agentd")
    parser.add_argument("--cycles", type=int, default=100)
    parser.add_argument("--max-rss-growth-mib", type=int, default=32)
    parser.add_argument("--max-p95-ms", type=int, default=5000)
    parser.add_argument("--output", default="artifacts/person3/agent-runtime-stress.json")
    args = parser.parse_args()
    assert 50 <= args.cycles <= 1000 and 1 <= args.max_rss_growth_mib <= 256
    with tempfile.TemporaryDirectory(prefix="cerynth-runtime-stress-") as root:
        root = Path(root)
        state, workspace = root / "state", root / "workspace"
        workspace.mkdir()
        daemon = subprocess.Popen([str(Path(args.binary).resolve()), "--directory", str(state), "serve"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 5
            while not (state / "agentd.sock").exists():
                if daemon.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("supervisor did not become ready")
                time.sleep(0.02)
            samples = []
            baseline = None
            for index in range(args.cycles + 10):
                start = time.monotonic()
                session = rpc(state, {"type": "create", "spec": {"name": "public-stress-fixture",
                    "harness": "generic_pty", "workspace": str(workspace), "model": {"name": "none"}}})
                session_id = session["id"]
                rpc(state, {"type": "start", "id": session_id, "operation_id": str(uuid.uuid4()),
                    "launch": {"program": "/usr/bin/true", "args": [], "initial_input": None, "pty": False}})
                deadline = time.monotonic() + 8
                while True:
                    status = rpc(state, {"type": "status", "id": session_id})
                    if status["state"] in ["completed", "failed", "cancelled"]:
                        assert status["state"] == "completed" and not status["execution"]["cleanup_pending"], status
                        break
                    if time.monotonic() > deadline:
                        raise TimeoutError("managed process cleanup stalled")
                    time.sleep(0.005)
                rpc(state, {"type": "archive", "id": session_id})
                assert rpc(state, {"type": "list"}) == []
                memory = rss(daemon.pid)
                if index == 9:
                    baseline = memory
                if index >= 10:
                    samples.append({"cycle": index - 10, "rss_bytes": memory,
                                    "lifecycle_ms": (time.monotonic() - start) * 1000})
            maximum = max(sample["rss_bytes"] for sample in samples)
            growth = max(0, maximum - baseline)
            p95 = percentile([sample["lifecycle_ms"] for sample in samples], 0.95)
            assert growth <= args.max_rss_growth_mib * 1024**2, "supervisor retained-memory regression"
            assert p95 <= args.max_p95_ms, "supervisor lifecycle-latency regression"
            report = {"kind": "managed-runtime-regression", "warmup_cycles": 10, "measured_cycles": args.cycles,
                "baseline_rss_bytes": baseline, "maximum_rss_bytes": maximum, "growth_bytes": growth,
                "p50_lifecycle_ms": percentile([s["lifecycle_ms"] for s in samples], 0.5), "p95_lifecycle_ms": p95,
                "rss_growth_limit_bytes": args.max_rss_growth_mib * 1024**2,
                "latency_limit_ms": args.max_p95_ms, "samples": samples,
                "limitations": ["Supervisor RSS only; not a formal proof of no leaks or a whole-system performance comparison",
                                "Archived operation metadata is intentionally retained; model/backend memory is not measured"]}
            output = Path(args.output)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps({key: report[key] for key in ["measured_cycles", "growth_bytes", "p95_lifecycle_ms"]}))
        finally:
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait()


if __name__ == "__main__":
    main()
