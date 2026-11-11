#!/usr/bin/env python3
"""Real loopback A2A subset fixture, with ephemeral sponsor-bound credentials."""
import argparse
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid
from harness_runtime_smoke import rpc
from guard_runtime_smoke import stop_process


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/cerynth-agentd")
    parser.add_argument("--output", default="artifacts/person3/a2a-runtime-smoke.json")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="cerynth-a2a-") as temporary:
        root = Path(temporary); workspace = root / "workspace"; workspace.mkdir()
        directory = root / "state"
        daemon = subprocess.Popen([str(Path(args.binary).resolve()), "--directory", str(directory), "serve"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        gateway = None
        try:
            deadline = time.monotonic() + 8
            while not (directory / "agentd.sock").exists():
                assert daemon.poll() is None and time.monotonic() < deadline
                time.sleep(.02)
            sponsor = rpc(directory, {"type":"create","spec":{"name":"sponsor", "harness":"generic_pty", "workspace":str(workspace),
                "model":{"name":"none"}, "runtime":{"capabilities":["task.delegate"],"budget":{
                    "memory_max_bytes":4*1024**3,"memory_high_bytes":3*1024**3,"inference_concurrency":4,"max_subagents":2}}}})
            with socket.socket() as reserved:
                reserved.bind(("127.0.0.1",0)); port = reserved.getsockname()[1]
            credential = directory / "gateway-auth" / "credential"
            exposed = root / "outside-control" / "credential"
            rejected = subprocess.run([str(Path(args.binary).resolve()), "--directory", str(directory), "a2a-gateway",
                "--listen",f"127.0.0.1:{port}", "--sponsor",sponsor["id"], "--credential-file",str(exposed)],
                stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10)
            assert rejected.returncode != 0 and not exposed.exists()
            gateway = subprocess.Popen([str(Path(args.binary).resolve()), "--directory", str(directory), "a2a-gateway",
                "--listen",f"127.0.0.1:{port}", "--sponsor",sponsor["id"], "--credential-file",str(credential)],
                stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            origin = f"http://127.0.0.1:{port}"
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
            deadline = time.monotonic() + 8
            while True:
                try:
                    with opener.open(origin+"/.well-known/agent-card.json",timeout=1) as reply: card = json.load(reply)
                    break
                except OSError:
                    assert gateway.poll() is None and time.monotonic() < deadline
                    time.sleep(.02)
            token = credential.read_text()
            assert credential.stat().st_mode & 0o777 == 0o600
            assert card["protocolVersion"] == "0.3.0" and card["capabilities"]["streaming"] is False
            assert token not in json.dumps(card) and sponsor["id"] not in json.dumps(card)

            def call(body, auth=token):
                headers = {"content-type":"application/json"}
                if auth is not None: headers["authorization"] = "Bearer "+auth
                request = urllib.request.Request(origin+"/a2a", data=json.dumps(body).encode(),headers=headers)
                try:
                    with opener.open(request,timeout=10) as reply: return reply.status,json.load(reply)
                except urllib.error.HTTPError as error: return error.code,None

            task_request = {"jsonrpc":"2.0","id":1,"method":"message/send","params":{"message":{
                "kind":"message","role":"user","messageId":str(uuid.uuid4()),"parts":[{"kind":"text","text":"public task fixture"}]}}}
            assert call(task_request, None)[0] == 401 and call(task_request,"wrong")[0] == 401
            result = call(task_request)[1]
            assert result["result"]["status"]["state"] == "submitted", result
            task = result["result"]["id"]
            assert call(task_request)[1]["result"]["id"] == task
            different = json.loads(json.dumps(task_request)); different["params"]["message"]["parts"][0]["text"] = "changed action"
            assert call(different)[1]["error"]["code"] == -32001
            assert call({"jsonrpc":"2.0","id":2,"method":"tasks/get","params":{"id":task}})[1]["result"]["id"] == task
            assert call({"jsonrpc":"2.0","id":3,"method":"tasks/get","params":{"id":sponsor["id"]}})[1]["error"]["code"] == -32001
            substituted = {**task_request,"params":{**task_request["params"],"sponsor":str(uuid.uuid4())}}
            assert call(substituted)[1]["error"]["code"] == -32602
            assert call({"jsonrpc":"2.0","id":4,"method":"message/stream","params":{}})[1]["error"]["code"] == -32601
            assert call({"jsonrpc":"2.0","id":5,"method":"tasks/cancel","params":{"id":task}})[1]["result"]["status"]["state"] == "canceled"
            child = rpc(directory,{"type":"status","id":task})
            assert child["execution"]["process"] is None and child["state"] == "cancelled"
            stop_process(gateway); gateway = None
            assert not credential.exists()
            report = {"kind":"real-loopback-a2a-subset-smoke","protocol":"0.3.0","agent_card":True,
                "sponsor_bound_authentication":True,"duplicate_message_deduplicated":True,"changed_message_rejected":True,
                "out_of_scope_task_denied":True,"sponsor_substitution_denied":True,"unsupported_streaming_rejected":True,
                "task_cancellation":True,"credential_removed_on_shutdown":True,"automatic_execution":False,
                "credential_outside_control_directory_rejected":True,
                "full_a2a_conformance":False,"cloud_requests":0}
            output=Path(args.output);output.parent.mkdir(parents=True,exist_ok=True);output.write_text(json.dumps(report,indent=2)+"\n")
            print(json.dumps(report,indent=2))
        finally:
            if gateway is not None: stop_process(gateway)
            stop_process(daemon)


if __name__ == "__main__": main()
