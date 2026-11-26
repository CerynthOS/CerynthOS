#!/usr/bin/env python3
"""Validate native OpenCode API discovery without acquiring process authority."""
import argparse
import base64
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid

from harness_runtime_smoke import rpc


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--opencode", required=True)
    parser.add_argument("--agentd", default="target/debug/cerynth-agentd")
    parser.add_argument("--output", default="artifacts/person3/opencode-api-smoke.json")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="cerynth-opencode-api-") as root:
        root = Path(root)
        workspace, home, state = root / "workspace", root / "home", root / "state"
        workspace.mkdir()
        home.mkdir(mode=0o700)
        with socket.socket() as reserved:
            reserved.bind(("127.0.0.1", 0))
            port = reserved.getsockname()[1]
        origin = f"http://127.0.0.1:{port}"
        password = uuid.uuid4().hex + uuid.uuid4().hex
        native_env = {"PATH": "/usr/bin:/bin", "HOME": str(home),
            "OPENCODE_SERVER_PASSWORD": password, "DO_NOT_TRACK": "1",
            "OPENCODE_DISABLE_AUTOUPDATE": "true", "OPENCODE_DISABLE_MODELS_FETCH": "true",
            "OPENCODE_CONFIG_CONTENT": json.dumps({"enabled_providers": []})}
        native = subprocess.Popen([str(Path(args.opencode).resolve()), "serve", "--pure",
                                   "--hostname", "127.0.0.1", "--port", str(port)],
                                  cwd=workspace, env=native_env, start_new_session=True,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        daemon = None
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

        def request(path, data=None, auth=True):
            headers = {"content-type": "application/json"}
            if auth:
                headers["authorization"] = "Basic " + base64.b64encode(f"opencode:{password}".encode()).decode()
            body = None if data is None else json.dumps(data).encode()
            with opener.open(urllib.request.Request(origin + path, data=body, headers=headers), timeout=2) as result:
                return json.load(result)

        try:
            deadline = time.monotonic() + 15
            while True:
                try:
                    health = request("/global/health")
                    break
                except (OSError, urllib.error.URLError):
                    if native.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError("native OpenCode API did not become ready")
                    time.sleep(0.05)
            try:
                request("/session", auth=False)
                raise AssertionError("native API unexpectedly allowed anonymous access")
            except urllib.error.HTTPError as error:
                assert error.code == 401
            created = request("/session", {"title": "public-native-api-fixture"})
            owner_env = os.environ.copy()
            owner_env["CERYNTH_OPENCODE_URL"] = origin
            owner_env["CERYNTH_OPENCODE_PASSWORD_ENV"] = "CERYNTH_NATIVE_TEST_PASSWORD"
            owner_env["CERYNTH_NATIVE_TEST_PASSWORD"] = password
            daemon = subprocess.Popen([str(Path(args.agentd).resolve()), "--directory", str(state), "serve"],
                                      env=owner_env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            deadline = time.monotonic() + 5
            while not (state / "agentd.sock").exists():
                if daemon.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("supervisor API did not become ready")
                time.sleep(0.02)
            discovery = rpc(state, {"type": "discover_harness", "harness": "open_code"})
            assert any(session["native_id"] == created["id"] for session in discovery["sessions"])
            assert discovery["control_acquired"] is False
            assert all(not session["imported"] and session["boundary"] == "unenforced_prototype"
                       for session in discovery["sessions"])
            assert rpc(state, {"type": "list"}) == []
            report = {"kind": "native-opencode-api-smoke", "version": health.get("version"),
                "authenticated_discovery_verified": True, "anonymous_access_rejected": True,
                "control_acquired": False, "registry_unchanged": True,
                "discovered_count": len(discovery["sessions"]), "inference_requests": 0,
                "limitations": ["Discovery only; native sessions are not automatically sandboxed or imported"]}
            output = Path(args.output)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report, indent=2))
        finally:
            if daemon is not None:
                daemon.terminate()
                try:
                    daemon.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    daemon.kill()
                    daemon.wait()
            if native.poll() is None:
                os.killpg(native.pid, signal.SIGTERM)
                try:
                    native.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(native.pid, signal.SIGKILL)
                    native.wait()


if __name__ == "__main__":
    main()
