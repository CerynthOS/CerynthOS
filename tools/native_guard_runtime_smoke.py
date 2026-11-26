#!/usr/bin/env python3
"""Exercise the pinned real Guard gateway with an adversarial synthetic harness.

The only upstream credential is a freshly generated local fixture canary. No
provider account, host credentials, downloaded model or model output is used.
"""
import argparse
import concurrent.futures
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import threading
import time
import uuid

from guard_runtime_smoke import stop_process
from harness_runtime_smoke import rpc
from inference_runtime_smoke import rpc as inference_rpc


def wait_until(check, seconds=15):
    deadline = time.monotonic() + seconds
    while True:
        result = check()
        if result:
            return result
        if time.monotonic() > deadline:
            raise TimeoutError("native Guard fixture condition timed out")
        time.sleep(.025)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--agentd", default="target/debug/cerynth-agentd")
    parser.add_argument("--inferenced", default="target/debug/cerynth-inferenced")
    parser.add_argument("--guardd", default="target/debug/cerynth-guardd")
    parser.add_argument("--guard-exec", default="target/debug/cerynth-guard-exec")
    parser.add_argument("--output", default="artifacts/person3/native-guard-runtime-smoke.json")
    args = parser.parse_args()
    canary = secrets.token_hex(32)
    received = []

    class Backend(BaseHTTPRequestHandler):
        def do_POST(self):
            assert self.headers.get("Authorization") == f"Bearer {canary}"
            assert self.path == "/v1/chat/completions"
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            assert body["model"] == "public-model"
            received.append(body)
            data = json.dumps({"choices": [{"message": {"role": "assistant", "content": "4"}}],
                               "usage": {"prompt_tokens": 4, "completion_tokens": 1}}).encode()
            self.send_response(200); self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data))); self.end_headers(); self.wfile.write(data)
        def log_message(self, *_): pass

    backend = ThreadingHTTPServer(("127.0.0.1", 0), Backend)
    thread = threading.Thread(target=backend.serve_forever, daemon=True); thread.start()
    with tempfile.TemporaryDirectory(prefix="cerynth-native-guard-") as temporary:
        root = Path(temporary)
        state, inference = root / "agent", root / "inference"
        with socket.socket() as reserved:
            reserved.bind(("127.0.0.1", 0)); proxy_port = reserved.getsockname()[1]
        sentinel = root / "host-sentinel"; sentinel.write_text("public host-only fixture")
        binaries = root / "native-bin"; binaries.mkdir()
        native = binaries / "native-probe"
        native.write_text('''#!/usr/bin/python3
import json,os,re,socket,sys,time,urllib.request,urllib.error
from pathlib import Path
if '--version' in sys.argv: print('public-native-probe-v1');sys.exit(0)
sys.stdin.read()
config=json.loads(os.environ['OPENCODE_CONFIG_CONTENT'])
options=config['provider']['ollama']['options']
assert options['apiKey']=='guard-owned'
assert not any(re.fullmatch('[a-f0-9]{64}',value) for value in os.environ.values())
assert 'CERYNTH_FIXTURE_PROVIDER_KEY' not in os.environ
opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
def call(path,body=None):
    req=urllib.request.Request(options['baseURL']+path,data=json.dumps(body).encode() if body is not None else None,
       headers={'Authorization':'Bearer client-forged-identity','Content-Type':'application/json'})
    try:
        with opener.open(req,timeout=5) as reply: return reply.status,json.load(reply)
    except urllib.error.HTTPError as e: return e.code,None
status,models=call('/models');assert status==200 and [m['id'] for m in models['data']]==['public-model']
assert call('/chat/completions',{'model':'public-model','messages':[{'role':'user','content':'2+2'}]})[0]==200
assert call('/chat/completions',{'model':'forbidden-model','messages':[{'role':'user','content':'2+2'}]})[0]==403
assert call('/admin')[0]==403
unknown=urllib.request.Request('http://127.0.0.1:8099/unknown/v1/models')
try: opener.open(unknown,timeout=3);raise AssertionError('unknown endpoint allowed')
except urllib.error.HTTPError as e: assert e.code==403
assert not Path(SENTINEL).exists() and not Path(CONTROL).exists()
for port in [BACKEND_PORT,PROXY_PORT]:
    try:
        with socket.create_connection(('127.0.0.1',port),timeout=.3): pass
        raise AssertionError('direct host network connection allowed')
    except OSError: pass
Path('/workspace/proof.json').write_text(json.dumps({'credential_hidden':True,'host_network_denied':True,
    'forged_identity_ignored':True,'model_scope_enforced':True,'admin_denied':True,'host_files_hidden':True}))
delay=30 if Path('/workspace/long').exists() else 2
time.sleep(delay)
print(json.dumps({'type':'step_finish','part':{'tokens':{'input':4,'output':1}}}))
'''.replace("SENTINEL", repr(str(sentinel))).replace("CONTROL", repr(str(inference / "control.sock")))
           .replace("BACKEND_PORT", str(backend.server_port)).replace("PROXY_PORT", str(proxy_port)))
        native.chmod(0o700)
        # Delay readiness to exercise cancellation during gateway provisioning.
        wrapper = root / "guard-wrapper"
        wrapper.write_text("#!/usr/bin/python3\nimport os,time\ntime.sleep(.7)\nos.execv(" +
                           repr(str(Path(args.guardd).resolve())) + ", [" + repr(str(Path(args.guardd).resolve())) + "] + __import__('sys').argv[1:])\n")
        wrapper.chmod(0o700)
        environment = {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(root),
            "CERYNTH_INFERENCE_DIR": str(inference), "CERYNTH_INFERENCE_HTTP": f"http://127.0.0.1:{proxy_port}",
            "CERYNTH_OPENCODE_BINARY": str(native), "CERYNTH_GUARDD_BINARY": str(wrapper),
            "CERYNTH_GUARD_EXEC_BINARY": str(Path(args.guard_exec).resolve())}
        coordinator = subprocess.Popen([str(Path(args.inferenced).resolve()), "--directory", str(inference),
            "serve", "--listen", f"127.0.0.1:{proxy_port}"], env={**environment, "CERYNTH_FIXTURE_PROVIDER_KEY": canary},
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        daemon = None
        identifiers = []
        try:
            wait_until(lambda: (inference / "control.sock").exists())
            inference_rpc(inference, True, {"type": "configure", "endpoint": {"kind": "compatible", "id": "local",
                "config": {"url": f"http://127.0.0.1:{backend.server_port}/v1", "cloud_consent": False,
                           "credential_env": "CERYNTH_FIXTURE_PROVIDER_KEY"}}})

            def launch_daemon():
                process = subprocess.Popen([str(Path(args.agentd).resolve()), "--directory", str(state), "serve"],
                    env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                def ready():
                    if process.poll() is not None:
                        raise RuntimeError(process.stderr.read().decode())
                    try: return rpc(state, {"type": "diagnostics"})
                    except OSError: return False
                wait_until(ready)
                return process

            daemon = launch_daemon()

            exposed = rpc(state, {"type":"create","spec":{"name":"control-overlap","harness":"generic_pty",
                "workspace":str(inference),"model":{"name":"none"}}})
            try:
                rpc(state,{"type":"start","id":exposed["id"],"operation_id":str(uuid.uuid4()),
                    "launch":{"program":"/usr/bin/true","args":[],"initial_input":None,"pty":False}})
                raise AssertionError("inference owner socket was exposed as a generic workspace")
            except RuntimeError as error: assert "control directory" in str(error)
            assert rpc(state,{"type":"status","id":exposed["id"]})["execution"]["process"] is None

            def create(name, long=False):
                workspace = root / name; workspace.mkdir()
                if long: (workspace / "long").touch()
                s = rpc(state, {"type": "create", "spec": {"name": name, "harness": "open_code", "workspace": str(workspace),
                    "model": {"name": "public-model", "endpoint_id": "local"}, "runtime": {
                    "guard": {"native_inference": True, "capabilities": [
                        {"kind": "fs.read", "target": "workspace"}, {"kind": "fs.write", "target": "workspace"},
                        {"kind": "fs.read", "target": str(binaries)},
                        {"kind": "inference.use", "target": "cerynth"},
                        {"kind": "credential.use", "target": "inference-session"}]},
                    "budget": {"timeout_seconds": 60, "max_subagents": 0}}}})
                identifiers.append(s["id"])
                return s, workspace

            def start(session):
                return rpc(state, {"type": "harness_start", "id": session["id"], "operation_id": str(uuid.uuid4()), "input": "public fixture"})

            def terminal(identifier):
                s = rpc(state, {"type": "status", "id": identifier})
                if s["state"] in ["completed", "failed", "cancelled"] and not any(s["execution"][key] for key in
                        ["cleanup_pending", "guard_cleanup_pending"]): return s
                return None

            a, wa = create("alpha"); b, wb = create("beta")
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                list(pool.map(start, [a, b]))
            for session, workspace in [(a, wa), (b, wb)]:
                result = wait_until(lambda: terminal(session["id"]))
                assert result["state"] == "completed", (result, rpc(state, {"type":"output","id":session["id"],"offset":0,"limit":4096}))
                assert all(json.loads((workspace / "proof.json").read_text()).values())
                assert not (state / "gateways" / session["id"] / "vault/inference-session.json").exists()
            metrics = inference_rpc(inference, True, {"type": "metrics"})
            assert {s["session_id"] for s in metrics["samples"]} == {a["id"], b["id"]}, metrics
            assert inference_rpc(inference, True, {"type": "health"})["grants"] == 0

            cancelled, wc = create("cancelled")
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                pending = pool.submit(start, cancelled)
                wait_until(lambda: rpc(state, {"type":"status","id":cancelled["id"]})["execution"]["native_guard_unit"])
                rpc(state, {"type": "stop", "id": cancelled["id"]})
                try: pending.result(timeout=15)
                except RuntimeError: pass
            result = wait_until(lambda: terminal(cancelled["id"]))
            assert result["state"] == "cancelled" and result["execution"]["process"] is None and not (wc / "proof.json").exists()

            crashed, wd = create("crashed", long=True); start(crashed)
            wait_until(lambda: (wd / "proof.json").exists())
            daemon.kill(); daemon.wait(timeout=10)
            daemon = launch_daemon()
            result = wait_until(lambda: terminal(crashed["id"]))
            assert result["state"] == "failed" and result["execution"]["operations"][0]["status"] == "unknown", result
            assert inference_rpc(inference, True, {"type": "health"})["grants"] == 0
            for identifier in identifiers:
                for unit in [f"cerynth-agent-{identifier}.scope", f"cerynth-guard-{identifier}.scope"]:
                    check = subprocess.run(["systemctl", "--user", "show", unit, "--property=LoadState", "--value"],
                                           capture_output=True, text=True, timeout=10)
                    assert check.stdout.strip() == "not-found", (unit, check.stdout)
            report = {"kind": "real-guard-native-inference-boundary-smoke", "guard_revision": "e592fa47797179ad24a99fc76143be48191a7649",
                "upstream": "local synthetic backend; generated canary credential", "harness": "adversarial synthetic OpenCode-compatible probe",
                "two_root_gateways": True, "provider_credential_hidden": True, "session_credential_hidden": True,
                "forged_agent_identity_ignored": True, "model_allowlist_enforced": True,
                "host_network_and_files_denied": True, "cancel_during_provisioning_has_no_execution": True,
                "generic_workspace_cannot_expose_inference_control": True,
                "supervisor_crash_recovers_scopes_and_revokes_grants": True, "vault_provisioning_files_removed": True,
                "session_cleanup": True, "cloud_requests": 0, "delegation_supported": False}
            output = Path(args.output); output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps(report, indent=2) + "\n"); print(json.dumps(report, indent=2))
        finally:
            if daemon is not None: stop_process(daemon)
            stop_process(coordinator)
            backend.shutdown(); backend.server_close(); thread.join(timeout=5)


if __name__ == "__main__": main()
