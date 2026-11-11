#!/usr/bin/env python3
"""Run an installed harness against public synthetic code and a local model."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import uuid


def rpc(directory, operation):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(10)
        stream.connect(str(directory / "agentd.sock"))
        stream.sendall(json.dumps({"version": 1, "operation": operation}).encode() + b"\n")
        response = json.loads(stream.makefile("rb").readline(4 * 1024 * 1024 + 1))
    result = response["result"]
    if "Err" in result:
        raise RuntimeError(result["Err"])
    return result["Ok"]


def verify_task(directory, workspace):
    """Run the independent checker inside a fresh managed boundary too."""
    checker = rpc(directory, {"type":"create","spec":{"name":"independent-checker","harness":"generic_pty",
        "workspace":str(workspace),"model":{"name":"none"},"runtime":{"budget":{"timeout_seconds":10}}}})
    nonce = uuid.uuid4().hex
    code = ("import importlib.util,json; "
            "s=importlib.util.spec_from_file_location('candidate','add.py'); "
            "m=importlib.util.module_from_spec(s);s.loader.exec_module(m); "
            f"print(json.dumps({{'nonce':{nonce!r},'results':[m.add(2,3),m.add(-1,1)]}}))")
    rpc(directory, {"type":"start","id":checker["id"],"operation_id":str(uuid.uuid4()),
        "launch":{"program":"/usr/bin/python3","args":["-I","-c",code],"initial_input":None,"pty":False}})
    deadline = time.monotonic() + 20
    while True:
        status = rpc(directory, {"type":"status","id":checker["id"]})
        if status["state"] in ["completed","failed","cancelled"]:
            break
        if time.monotonic() > deadline:
            raise TimeoutError("independent checker did not terminate")
        time.sleep(0.02)
    output = rpc(directory, {"type":"output","id":checker["id"],"offset":0,"limit":32768})
    receipt = None
    for line in output["text"].splitlines():
        try:
            candidate = json.loads(line)
            if isinstance(candidate, dict) and candidate.get("nonce") == nonce:
                receipt = candidate
        except ValueError:
            pass
    passed = (status["state"] == "completed" and status["execution"]["exit_code"] == 0
              and isinstance(receipt, dict) and receipt.get("results") == [5,0])
    return {"session_id":checker["id"],"boundary":status["boundary"],"state":status["state"],
            "fixed_cases_passed":passed,"case_count":2,"network":"deny"}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/cerynth-agentd")
    parser.add_argument("--harness", choices=["codex", "open_code", "hermes"], default="codex")
    parser.add_argument("--model", default="qwen3:0.6b")
    parser.add_argument("--timeout", type=int, default=90)
    parser.add_argument("--output", default="artifacts/person3/harness-smoke.json")
    parser.add_argument("--brokered", action="store_true")
    parser.add_argument("--inference-binary", default="target/debug/cerynth-inferenced")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="cerynth-harness-smoke-") as root:
        root = Path(root)
        state, workspace = root / "state", root / "workspace"
        workspace.mkdir()
        subprocess.run(["git", "init", "--quiet", str(workspace)], check=True)
        (workspace / "add.py").write_text("def add(a,b): return a-b\n")
        owner_env = os.environ.copy()
        inference_daemon = None
        inference_directory = root / "inference"
        if args.brokered:
            from inference_runtime_smoke import rpc as inference_rpc
            with socket.socket() as reserved:
                reserved.bind(("127.0.0.1", 0))
                proxy_port = reserved.getsockname()[1]
            inference_daemon = subprocess.Popen([str(Path(args.inference_binary).resolve()), "--directory",
                str(inference_directory), "serve", "--listen", f"127.0.0.1:{proxy_port}"],
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            deadline = time.monotonic() + 5
            while not (inference_directory / "control.sock").exists():
                if inference_daemon.poll() is not None or time.monotonic() > deadline:
                    inference_daemon.terminate()
                    inference_daemon.wait(timeout=5)
                    raise RuntimeError("inference coordinator did not start")
                time.sleep(0.02)
            inference_rpc(inference_directory, True, {"type": "configure", "endpoint": {
                "kind": "ollama", "id": "local", "url": "http://127.0.0.1:11434"}})
            owner_env["CERYNTH_INFERENCE_DIR"] = str(inference_directory)
            owner_env["CERYNTH_INFERENCE_HTTP"] = f"http://127.0.0.1:{proxy_port}"
        daemon = subprocess.Popen([str(Path(args.binary).resolve()), "--directory", str(state), "serve"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=owner_env)
        try:
            deadline = time.monotonic() + 5
            while not (state / "agentd.sock").exists():
                if time.monotonic() > deadline or daemon.poll() is not None:
                    raise RuntimeError("agent daemon failed to start")
                time.sleep(0.02)
            session = rpc(state, {"type": "create", "spec": {
                "name": "local-harness-smoke", "harness": args.harness,
                "workspace": str(workspace), "model": {"name": args.model,"disable_thinking": True,
                    "endpoint_id": "local" if args.brokered else None},
                "runtime": {"network": "host", "budget": {"timeout_seconds": args.timeout}}}})
            session_id = session["id"]
            before = inference_rpc(inference_directory, True, {"type":"registry","endpoint_id":"local","model":args.model}) if args.brokered else None
            rpc(state, {"type": "harness_start", "id": session_id, "operation_id": str(uuid.uuid4()),
                        "input": f"Use the available tools to read and edit the exact file {workspace / 'add.py'}. Its add(a,b) function currently subtracts. Fix it to return a+b, then run python3 to verify add(2,3)==5. Execute the edit and test using tools. Do not use the network or describe commands for me to execute."})
            deadline = time.monotonic() + args.timeout + 15
            while True:
                status = rpc(state, {"type": "status", "id": session_id})
                if status["state"] in ["completed", "failed", "cancelled"]:
                    break
                if time.monotonic() > deadline:
                    raise TimeoutError("harness supervisor did not terminate")
                time.sleep(0.1)
            verification = verify_task(state, workspace)
            output = rpc(state, {"type": "output", "id": session_id, "offset": 0, "limit": 32768})
            report = {"kind": "real-local-harness-smoke", "harness": args.harness, "model": args.model,
                      "state": status["state"], "exit_code": status["execution"]["exit_code"],
                      "coding_task_verified": verification["fixed_cases_passed"], "verification":verification,"boundary": status["boundary"],
                      "events": status["events"], "harness_events": status["execution"]["harness_events"],
                      "synthetic_task_output": output["text"],
                      "inference_metrics": inference_rpc(inference_directory, True, {"type":"metrics"}) if args.brokered else None,
                      "model_observations": {"before":before,"after":inference_rpc(inference_directory, True,
                          {"type":"registry","endpoint_id":"local","model":args.model})} if args.brokered else None,
                      "network_policy": "explicit host networking to reach local Ollama; no egress isolation claim"}
            dest = Path(args.output)
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps({k: report[k] for k in ["harness", "state", "coding_task_verified", "exit_code"]}, indent=2))
        finally:
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait()
            if inference_daemon is not None:
                inference_daemon.terminate()
                try:
                    inference_daemon.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    inference_daemon.kill()
                    inference_daemon.wait()


if __name__ == "__main__":
    main()
