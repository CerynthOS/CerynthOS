#!/usr/bin/env python3
"""Exercise isolated checkout with hostile Git hooks and filters on public fixtures."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import time
import uuid

from harness_runtime_smoke import rpc


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/cerynth-agentd")
    parser.add_argument("--output", default="artifacts/person3/worktree-runtime-smoke.json")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="cw-") as temporary:
        root = Path(temporary)
        source, state = root / "repo", root / "state"
        source.mkdir()

        def git(*arguments):
            return subprocess.run(["git", "-C", str(source), *arguments], check=True,
                                  capture_output=True, text=True, timeout=10).stdout.strip()

        git("init", "--quiet")
        (source / "hello.txt").write_text("public fixture\n")
        (source / ".gitattributes").write_text("*.txt filter=untrusted\n")
        git("add", ".")
        git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
            "-c", "core.hooksPath=/dev/null", "commit", "--quiet", "-m", "public fixture")
        hook_marker, filter_marker = root / "hook-ran", root / "filter-ran"
        hooks = source / ".git" / "hooks"
        hook = hooks / "post-checkout"
        hook.write_text(f"#!/bin/sh\nprintf hook > hook-in-checkout\nprintf hook > '{hook_marker}'\n")
        hook.chmod(0o755)
        git("config", "filter.untrusted.smudge", f"sh -c 'echo filter > {filter_marker}; echo FILTER_RAN; cat'")
        git("config", "filter.untrusted.required", "true")
        original_config = (source / ".git" / "config").read_bytes()
        daemon = subprocess.Popen([str(Path(args.binary).resolve()), "--directory", str(state), "serve"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 5
            while not (state / "agentd.sock").exists():
                if daemon.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("agentd did not start")
                time.sleep(0.02)
            session = rpc(state, {"type": "create", "spec": {"name": "isolated-checkout",
                "harness": "generic_pty", "workspace": str(source), "model": {"name": "none"}}})
            prepared = rpc(state, {"type": "worktree", "id": session["id"]})
            worktree = Path(prepared["path"])
            assert (worktree / "hello.txt").read_text() == "public fixture\n", prepared
            assert not hook_marker.exists() and not filter_marker.exists()
            assert not (worktree / "hook-in-checkout").exists()
            assert not (source / "hook-in-checkout").exists()
            assert (source / ".git" / "config").read_bytes() == original_config
            again = rpc(state, {"type": "worktree", "id": session["id"]})
            assert again["existing"] and again["path"] == prepared["path"]
            ready = rpc(state, {"type": "status", "id": session["id"]})
            assert ready["state"] == "preparing" and not ready["execution"]["cleanup_pending"]
            rpc(state, {"type": "start", "id": session["id"], "operation_id": str(uuid.uuid4()),
                "launch": {"program": "/usr/bin/python3", "args": ["-I", "-c",
                    "from pathlib import Path; assert Path('hello.txt').read_text() == 'public fixture\\n'; Path('result.txt').write_text('managed')"],
                    "initial_input": None, "pty": False}})
            deadline = time.monotonic() + 15
            while True:
                result = rpc(state, {"type": "status", "id": session["id"]})
                if result["state"] in ["completed", "failed", "cancelled"]:
                    break
                if time.monotonic() > deadline:
                    raise TimeoutError("prepared worktree session did not terminate")
                time.sleep(0.02)
            assert result["state"] == "completed", result
            assert (worktree / "result.txt").read_text() == "managed"
            assert not (source / "result.txt").exists()
            report = {"kind": "real-host-worktree-smoke", "session_id": session["id"],
                "checkout_filters": prepared["checkout_filters"], "hook_not_executed": True,
                "filter_not_executed": True, "source_config_unchanged": True,
                "repeat_returns_existing": True, "prepared_session_launches": True,
                "source_workspace_unchanged": True, "boundary": result["boundary"],
                "state": result["state"], "cloud_requests": 0}
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
