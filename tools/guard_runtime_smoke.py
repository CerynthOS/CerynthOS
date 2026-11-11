#!/usr/bin/env python3
"""Real P3/P2 integration using Guard commit e592fa4, with no cloud credentials.

Build the Guard binaries from that branch separately. This does not merge or
modify Person 2's implementation, install a service, or approve an agent action.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import uuid

from harness_runtime_smoke import rpc


def guard_rpc(path, operation):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(5)
        stream.connect(str(path))
        stream.sendall(json.dumps(operation).encode() + b"\n")
        return json.loads(stream.makefile("rb").readline(1024 * 1024 + 1))


def stop_process(process):
    process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--agentd", default="target/debug/cerynth-agentd")
    parser.add_argument("--guardd", default="target/debug/cerynth-guardd")
    parser.add_argument("--guard-exec", default="target/debug/cerynth-guard-exec")
    parser.add_argument("--output", default="artifacts/person3/guard-runtime-smoke.json")
    args = parser.parse_args()
    # Guard's per-session Unix socket names include a UUID; keep the parent
    # short enough for Linux's sockaddr_un limit, including the egress suffix.
    with tempfile.TemporaryDirectory(prefix="cg-") as root:
        root = Path(root)
        state, guard_run = root / "agent-state", root / "guard-run"
        workspace = root / "workspace"
        workspace.mkdir()
        sentinel = root / "host-only.txt"
        sentinel.write_text("public host-only fixture")
        config = root / "guard.toml"
        config.write_text(f"runtime_dir = {json.dumps(str(guard_run))}\nstate_dir = {json.dumps(str(root / 'guard-state'))}\n"
                          f"guard_exec = {json.dumps(str(Path(args.guard_exec).resolve()))}\n")
        guard = None
        daemon = None
        control = guard_run / "guard.sock"

        def launch_guard():
            process = subprocess.Popen([str(Path(args.guardd).resolve()), "serve", str(config)],
                                       env={"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(root)},
                                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            deadline = time.monotonic() + 8
            while True:
                try:
                    if guard_rpc(control, {"op": "status"}).get("type") == "status":
                        return process
                except (OSError, ValueError):
                    pass
                if process.poll() is not None or time.monotonic() > deadline:
                    stop_process(process)
                    raise RuntimeError("Guard did not become ready")
                time.sleep(0.02)

        def create(path, parent=None, capabilities=None, budget=None):
            path.mkdir(exist_ok=True)
            return rpc(state, {"type": "create", "spec": {"name": "guard-public-fixture", "harness": "generic_pty",
                "workspace": str(path), "model": {"name": "none"}, "runtime": {"parent_id": parent,
                    "guard": {"capabilities": capabilities or []}, "budget": budget or {}}}})["id"]

        def start(session_id, program, arguments):
            return rpc(state, {"type": "start", "id": session_id, "operation_id": str(uuid.uuid4()),
                "launch": {"program": program, "args": arguments, "initial_input": None, "pty": False}})

        def terminal(session_id):
            deadline = time.monotonic() + 12
            while True:
                session = rpc(state, {"type": "status", "id": session_id})
                if session["state"] in ["completed", "failed", "cancelled"]:
                    if session["execution"]["guard_cleanup_pending"] or session["execution"]["cleanup_pending"]:
                        rpc(state, {"type": "cleanup", "id": session_id})
                    else:
                        return session
                if time.monotonic() > deadline:
                    raise TimeoutError(json.dumps({"message":"process or Guard authorization cleanup stalled","session":session,
                        "output":rpc(state,{"type":"output","id":session_id,"offset":0,"limit":4096})}))
                time.sleep(0.02)

        try:
            guard = launch_guard()
            daemon = subprocess.Popen([str(Path(args.agentd).resolve()), "--directory", str(state), "serve"],
                env={"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "CERYNTH_GUARD_SOCKET": str(control)},
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            deadline = time.monotonic() + 5
            while not (state / "agentd.sock").exists():
                if daemon.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("supervisor did not become ready")
                time.sleep(0.02)
            session_id = create(workspace)
            code = (
                "import json,os,pathlib,socket; "
                f"hidden=not pathlib.Path({str(sentinel)!r}).exists(); "
                "status=pathlib.Path('/proc/self/status').read_text(); "
                "s=socket.socket(); s.settimeout(.5); denied=s.connect_ex(('127.0.0.1',11434))!=0;s.close(); "
                "g=socket.socket(socket.AF_UNIX);g.settimeout(2);g.connect(os.environ['CERYNTH_GUARD_SOCKET']);"
                "g.sendall(b'{\"op\":\"pending_approvals\"}\\n');reply=json.loads(g.makefile('rb').readline());g.close(); "
                "e={'host_file_hidden':hidden,'network_denied':denied,'seccomp_filter': 'Seccomp:\\t2' in status,"
                "'agent_cannot_approve':reply.get('code')=='caller_not_authorized','cwd':os.getcwd()}; "
                "pathlib.Path('guard-result.json').write_text(json.dumps(e));assert all(e[k] for k in e if k!='cwd')"
            )
            start(session_id, "/usr/bin/python3", ["-c", code])
            finished = terminal(session_id)
            assert finished["state"] == "completed", rpc(state, {"type": "output", "id": session_id, "offset": 0, "limit": 32768})
            assert finished["boundary"] == "guard_systemd"
            evidence = json.loads((workspace / "guard-result.json").read_text())
            assert evidence["cwd"] == "/workspace"
            assert guard_rpc(control, {"op": "status"})["sessions"] == 0

            parent = create(root / "parent", capabilities=[{"kind": "session.spawn", "target": "child", "limit": 1}],
                            budget={"memory_max_bytes": 4 * 1024**3, "inference_concurrency": 4})
            start(parent, "/usr/bin/sleep", ["60"])
            bad = create(root / "bad-child", parent=parent, capabilities=[{"kind": "gpu.use", "target": "*"}])
            try:
                start(bad, "/usr/bin/true", [])
                raise AssertionError("Guard allowed child capability escalation")
            except RuntimeError as error:
                assert "Guard denied" in str(error)
            assert terminal(bad)["state"] == "failed"
            child = create(root / "child", parent=parent)
            start(child, "/usr/bin/sleep", ["60"])
            rpc(state, {"type": "stop", "id": parent})
            assert terminal(child)["state"] == "cancelled"
            assert terminal(parent)["state"] == "cancelled"
            assert guard_rpc(control, {"op": "status"})["sessions"] == 0

            stop_process(guard)
            guard = None
            offline = create(root / "offline")
            marker = root / "offline" / "should-not-exist"
            try:
                start(offline, "/usr/bin/touch", ["/workspace/should-not-exist"])
                raise AssertionError("unavailable Guard was bypassed")
            except RuntimeError:
                pass
            assert not marker.exists()
            guard = launch_guard()
            rpc(state, {"type": "cleanup", "id": offline})
            assert not terminal(offline)["execution"]["guard_cleanup_pending"]
            report = {"kind": "p3-p2-real-guard-integration", "guard_revision": "e592fa47797179ad24a99fc76143be48191a7649",
                "boundary": finished["boundary"], "profile_digest": finished["execution"]["guard_profile_digest"],
                "checks": evidence, "child_escalation_denied": True, "child_before_parent_cleanup": True,
                "unavailable_guard_no_fallback": True, "authorization_cleanup_verified": True,
                "cloud_requests": 0, "limitations": ["Offline generic commands only; native inference gateway bridge requires shared contract review"]}
            output = Path(args.output)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report, indent=2))
        finally:
            if daemon is not None:
                stop_process(daemon)
            if guard is not None:
                stop_process(guard)


if __name__ == "__main__":
    main()
