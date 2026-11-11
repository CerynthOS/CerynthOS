#!/usr/bin/env python3
"""Exercise owner-only placement advice with measured Ollama switching costs."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import time

from inference_runtime_smoke import rpc


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/cerynth-inferenced")
    parser.add_argument("--baseline", default="artifacts/person3/cache-baseline.json")
    parser.add_argument("--output", default="artifacts/person3/placement-runtime-smoke.json")
    args = parser.parse_args()
    raw = Path(args.baseline).read_bytes(); baseline = json.loads(raw)
    model = next(m for m in baseline["registry"]["models"] if m["name"] == "qwen3:0.6b")
    memory = subprocess.run(["nvidia-smi", "--query-gpu=memory.total,memory.used", "--format=csv,noheader,nounits"],
                            capture_output=True, text=True, check=True, timeout=5).stdout.splitlines()
    assert len(memory) == 1
    total, used = [int(v.strip()) * 1024**2 for v in memory[0].split(",")]
    target = {"id": "local-gpu", "trust_domain": "public-test", "calibration_id": "ollama-0.35-context16384-single-host",
        "total_bytes": total, "free_bytes": total - used, "reserved_bytes": 0, "desktop_margin_bytes": 1024**3,
        "observed_at_ms": int(time.time() * 1000), "switch_supported": True, "resident": None}
    observations = [{"target_id": target["id"], "calibration_id": target["calibration_id"], "model_digest": model["digest"],
        "context_length": baseline["context_length"], "load_ms": item["engine_counters"]["load_duration"] / 1e6}
        for item in baseline["switch_trials"] if item["model"] == model["name"]]
    operation = {"type": "plan_placement", "request": {"model_digest": model["digest"],
        "context_length": baseline["context_length"], "resident_bytes": model["allocated_vram_bytes"],
        "trust_domain": "public-test", "authorized_targets": [target["id"]]}, "targets": [target], "observations": observations}
    with tempfile.TemporaryDirectory(prefix="cerynth-placement-") as temporary:
        directory = Path(temporary)
        daemon = subprocess.Popen([str(Path(args.binary).resolve()), "--directory", str(directory), "serve"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 8
            while not (directory / "control.sock").exists():
                if daemon.poll() is not None or time.monotonic() > deadline: raise RuntimeError("coordinator did not start")
                time.sleep(.02)
            result = rpc(directory, True, operation)
            assert result["selected"] == "local-gpu" and result["changed_system_state"] is False, result
            estimate = result["assessments"][0]["switch_estimate"]
            assert estimate["samples"] == len(observations)
            protected = copy.deepcopy(operation)
            protected["targets"][0]["resident"] = {"digest": "another-model", "context_length": 16384,
                "allocated_bytes": 1024**3, "active_requests": 1, "protected": False}
            protected_result = rpc(directory, True, protected)
            assert protected_result["selected"] is None
            stale = copy.deepcopy(operation); stale["targets"][0]["observed_at_ms"] -= 60001
            assert rpc(directory, True, stale)["selected"] is None
            alien = copy.deepcopy(operation); alien["targets"][0]["trust_domain"] = "other-owner"
            assert rpc(directory, True, alien)["selected"] is None
            try:
                rpc(directory, False, operation)
                raise AssertionError("data plane accepted an administrative placement request")
            except RuntimeError: pass
            report = {"kind": "measured-cost-owner-placement-smoke", "model": model["name"],
                "baseline_sha256": hashlib.sha256(raw).hexdigest(), "measured_load_ms": [v["load_ms"] for v in observations],
                "required_resident_bytes": model["allocated_vram_bytes"], "weight_file_bytes": model["weight_bytes"],
                "plan": result, "active_model_protected": True, "stale_telemetry_queued": True,
                "cross_domain_target_denied": True, "data_plane_cannot_plan": True,
                "cloud_requests": 0, "physical_model_placement_changed": False}
            dest = Path(args.output); dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(json.dumps(report, indent=2) + "\n"); print(json.dumps(report, indent=2))
        finally:
            daemon.terminate()
            try: daemon.wait(timeout=8)
            except subprocess.TimeoutExpired: daemon.kill(); daemon.wait()


if __name__ == "__main__": main()
