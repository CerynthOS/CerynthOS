#!/usr/bin/env python3
"""Real systemd/Bubblewrap smoke test. No model download or cloud access."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/cerynth-agentd")
    parser.add_argument("--output", default="artifacts/person3/runtime-smoke.json")
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    with tempfile.TemporaryDirectory(prefix="cerynth-runtime-smoke-") as root:
        root = Path(root)
        state, workspace = root / "state", root / "workspace"
        workspace.mkdir()
        sentinel = root / "private.txt"
        sentinel.write_text("host-only sentinel")
        daemon = subprocess.Popen([binary, "--directory", str(state), "serve"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        def call(*command):
            result = subprocess.run([binary, "--directory", str(state), *command],
                                    capture_output=True, text=True, timeout=20, check=True)
            return json.loads(result.stdout)
        try:
            deadline = time.monotonic() + 5
            while not (state / "agentd.sock").exists():
                if daemon.poll() is not None:
                    raise RuntimeError(daemon.stderr.read().decode())
                if time.monotonic() > deadline:
                    raise TimeoutError("daemon did not bind")
                time.sleep(0.02)
            session = call("create", "--name", "sandbox-scope-smoke", "--workspace", str(workspace),
                           "--model", "none", "--harness", "generic_pty")
            session_id = session["id"]
            code = (
                "import json, pathlib, socket, time; "
                f"hidden = not pathlib.Path({str(sentinel)!r}).exists(); "
                "s=socket.socket(); s.settimeout(0.5); denied=s.connect_ex(('127.0.0.1',11434)) != 0; s.close(); "
                "cg=pathlib.Path('/proc/self/cgroup').read_text(); "
                "pathlib.Path('sandbox-result.json').write_text(json.dumps({'host_file_hidden':hidden,'network_denied':denied,'cgroup':cg})); "
                "assert hidden and denied; time.sleep(0.5)"
            )
            launched = call("start", session_id, "--operation-id", str(uuid.uuid4()), "/usr/bin/python3", "-c", code)
            unit = f"cerynth-agent-{session_id}.scope"
            properties = ""
            for _ in range(20):
                probe = subprocess.run(["systemctl", "--user", "show", unit, "--property=MemoryMax,CPUWeight,TasksMax"],
                                       capture_output=True, text=True, timeout=3)
                if "MemoryMax=2147483648" in probe.stdout:
                    properties = probe.stdout
                    break
                time.sleep(0.02)
            deadline = time.monotonic() + 15
            while True:
                status = call("status", session_id)
                if status["state"] in ("completed", "failed", "cancelled"):
                    break
                if time.monotonic() > deadline:
                    raise TimeoutError("managed process did not finish")
                time.sleep(0.02)
            if status["state"] != "completed":
                raise RuntimeError(json.dumps({"status": status, "output": call("output", session_id)}))
            evidence = json.loads((workspace / "sandbox-result.json").read_text())
            assert "cerynth-agent-" in evidence["cgroup"], evidence
            assert "MemoryMax=2147483648" in properties, properties
            assert sentinel.read_text() == "host-only sentinel"
            report = {"kind": "real-host-smoke", "session_id": session_id, "launch": launched,
                      "state": status["state"], "boundary": status["boundary"], "checks": evidence,
                      "systemd_properties": properties.splitlines(), "cloud_requests": 0}
            crashed = call("create", "--name", "managed-crash-smoke", "--workspace", str(workspace),
                           "--model", "none", "--harness", "generic_pty")
            crash_id = crashed["id"]
            call("start", crash_id, "/usr/bin/sh", "-c", "sleep 60 & wait")
            crash_unit = f"cerynth-agent-{crash_id}.scope"
            deadline = time.monotonic() + 5
            while True:
                probe = subprocess.run(["systemctl", "--user", "show", crash_unit, "--property=ControlGroup", "--value"],
                                       capture_output=True, text=True, timeout=3)
                group = probe.stdout.strip()
                if group and (Path("/sys/fs/cgroup") / group.lstrip("/") / "cgroup.procs").exists():
                    break
                if time.monotonic() > deadline:
                    raise TimeoutError("crash-test scope did not become ready")
                time.sleep(0.02)
            cgroup = Path("/sys/fs/cgroup") / group.lstrip("/")
            pids = (cgroup / "cgroup.procs").read_text().split()
            assert pids, "managed crash fixture has no processes"
            daemon.kill()
            daemon.wait(timeout=5)
            daemon = subprocess.Popen([binary, "--directory", str(state), "serve"],
                                      stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            deadline = time.monotonic() + 8
            while True:
                try:
                    recovered = call("status", crash_id)
                    break
                except subprocess.CalledProcessError:
                    if daemon.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError("managed crash recovery did not become ready")
                    time.sleep(0.05)
            assert recovered["state"] == "failed" and not recovered["execution"]["cleanup_pending"]
            assert not cgroup.exists() or "populated 0" in (cgroup / "cgroup.events").read_text()
            for pid in pids:
                stat = Path(f"/proc/{pid}/stat")
                assert not stat.exists() or ") Z " in stat.read_text(), "owned scope process survived restart"
            report["managed_crash_recovery"] = {"state": recovered["state"], "scope_empty": True,
                "owned_processes_stopped": True, "operation_status": recovered["execution"]["operations"][0]["status"]}
            output = Path(args.output)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report, indent=2))
        finally:
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait()


if __name__ == "__main__":
    main()
