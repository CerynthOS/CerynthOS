#!/usr/bin/env python3
"""Actual Guard-authorized, supervised CUDA execution and lease cleanup."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import uuid

from guard_runtime_smoke import guard_rpc, stop_process
from harness_runtime_smoke import rpc


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--agentd", default="target/debug/cerynth-agentd")
    parser.add_argument("--guardd", default="target/debug/cerynth-guardd")
    parser.add_argument("--guard-exec", default="target/debug/cerynth-guard-exec")
    parser.add_argument("--output", default="artifacts/person3/gpu-runtime-smoke.json")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="gg-") as temporary:
        root = Path(temporary)
        state, guard_run = root / "a", root / "g"
        config = root / "guard.toml"
        config.write_text(f"runtime_dir = {json.dumps(str(guard_run))}\nstate_dir = {json.dumps(str(root/'s'))}\n"
                          f"guard_exec = {json.dumps(str(Path(args.guard_exec).resolve()))}\n")
        guard = subprocess.Popen([str(Path(args.guardd).resolve()), "serve", str(config)],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        daemon = None
        try:
            deadline = time.monotonic() + 8
            while True:
                try:
                    if guard_rpc(guard_run / "guard.sock", {"op": "status"})["type"] == "status": break
                except (OSError, ValueError): pass
                if guard.poll() is not None or time.monotonic() > deadline: raise RuntimeError("Guard did not start")
                time.sleep(.02)
            owner_env = os.environ.copy()
            owner_env["CERYNTH_GUARD_SOCKET"] = str(guard_run / "guard.sock")
            daemon = subprocess.Popen([str(Path(args.agentd).resolve()), "--directory", str(state), "serve"],
                                      env=owner_env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            deadline = time.monotonic() + 8
            while not (state / "agentd.sock").exists():
                if daemon.poll() is not None or time.monotonic() > deadline: raise RuntimeError("agentd did not start")
                time.sleep(.02)
            inventory = rpc(state, {"type": "inventory"})
            device = next(d for d in inventory["gpus"] if d["vendor"] == "nvidia")

            def create(name, allow_gpu=True, timeout=8):
                workspace = root / name
                workspace.mkdir()
                shutil.copyfile(Path(__file__).with_name("cuda_contention.py"), workspace / "cuda.py")
                caps = [{"kind": "fs.read", "target": "workspace"}, {"kind": "fs.write", "target": "workspace"}]
                if allow_gpu:
                    caps.append({"kind": "gpu.use", "target": "*"})
                    caps.append({"kind": "fs.read", "target": "/sys"})
                session = rpc(state, {"type": "create", "spec": {"name": name, "harness": "generic_pty",
                    "workspace": str(workspace), "model": {"name": "none"}, "runtime": {
                    "capabilities": ["gpu.execute"], "guard": {"capabilities": caps},
                    "budget": {"gpu_memory_bytes": 256 * 1024**2, "timeout_seconds": timeout}}}})
                job = rpc(state, {"type": "gpu_submit", "id": session["id"], "device_id": device["id"],
                                  "memory_bytes": 256 * 1024**2, "priority": 0})
                assert job["state"] == "admitted", job
                return session, job, workspace

            def run(session, job, seconds):
                op = {"type": "gpu_run", "id": session["id"], "job_id": job["id"], "operation_id": str(uuid.uuid4()),
                    "launch": {"program": "/usr/bin/python3", "args": ["-I", "/workspace/cuda.py", "--seconds", str(seconds)],
                               "initial_input": None, "pty": False}}
                rpc(state, op)
                return op

            def terminal(session, job):
                deadline = time.monotonic() + 15
                while True:
                    result = rpc(state, {"type": "status", "id": session["id"]})
                    if result["state"] in ["completed", "failed", "cancelled"] and not result["execution"]["gpu_cleanup_pending"]: break
                    if time.monotonic() > deadline: raise TimeoutError("GPU boundary did not clean up")
                    time.sleep(.02)
                jobs = rpc(state, {"type": "gpu_jobs"})
                return result, jobs["jobs"][job["id"]]

            session, job, _ = create("cuda")
            operation = run(session, job, 3)
            try:
                rpc(state, {"type": "gpu_finish", "id": session["id"], "job_id": job["id"], "succeeded": True})
                raise AssertionError("manual release bypassed owned cleanup")
            except RuntimeError as error:
                assert "verified process cleanup" in str(error), str(error)
            result, finished = terminal(session, job)
            output = rpc(state, {"type": "output", "id": session["id"], "offset": 0, "limit": 32768})["text"]
            assert result["state"] == "completed" and finished["state"] == "completed", (result, output)
            assert '"ready": true' in output and '"allocated_bytes": 67108864' in output, output
            assert any(e["reason"] == "guard_gpu_authorized" for e in result["events"])
            assert result["execution"]["gpu_authorization"]["launch_sha256"] == result["execution"]["operations"][0]["fingerprint"]
            assert rpc(state, operation)["replayed"] is True
            changed = {**operation, "launch": {**operation["launch"], "args": operation["launch"]["args"] + ["--memory-mib", "1"]}}
            try:
                rpc(state, changed)
                raise AssertionError("operation replay allowed substituted launch details")
            except RuntimeError as error:
                assert "different details" in str(error), str(error)
            timeout_session, timeout_job, _ = create("deadline", timeout=2)
            run(timeout_session, timeout_job, 30)
            timed, timed_job = terminal(timeout_session, timeout_job)
            assert timed["state"] == "failed" and timed["execution"]["failure_reason"] == "process_timeout", timed
            assert timed_job["state"] == "failed"
            denied, denied_job, workspace = create("denied", allow_gpu=False)
            try:
                run(denied, denied_job, 1)
                raise AssertionError("Guard allowed missing GPU capability")
            except RuntimeError as error:
                assert "Guard did not authorize" in str(error), str(error)
            rejected, rejected_job = terminal(denied, denied_job)
            assert rejected_job["state"] == "failed" and rejected["execution"]["process"] is None
            assert not (workspace / "executed").exists()
            deadline = time.monotonic() + 3
            while guard_rpc(guard_run / "guard.sock", {"op":"status"})["sessions"]:
                if time.monotonic() > deadline: raise TimeoutError("Guard authority cleanup did not finish")
                time.sleep(.02)
            report = {"kind": "real-guard-supervised-cuda-smoke", "boundary": result["boundary"],
                "allocated_bytes": 67108864, "cooperative_reservation_bytes": 256 * 1024**2,
                "guard_gpu_authorized": True, "manual_lease_release_denied": True,
                "guard_policy_enforcement": result["execution"]["gpu_authorization"]["policy_enforcement"],
                "runtime_launch_gate": "enforced for owned GPU execution",
                "changed_launch_replay_denied": True,
                "duplicate_launch_not_replayed": True, "deadline_stops_process": True,
                "missing_guard_capability_denied": True, "process_and_reservation_cleanup": True,
                "cloud_requests": 0, "hard_vram_limit": False,
                "device_access": "Guard GPU profile exposes all supported device nodes; no per-device partition"}
            dest = Path(args.output); dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report, indent=2))
        finally:
            if daemon is not None: stop_process(daemon)
            stop_process(guard)


if __name__ == "__main__": main()
