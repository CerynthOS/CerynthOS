#!/usr/bin/env python3
"""Owner-sponsored OpenCode/Hermes routing over isolated worktrees and one local model.

Requires explicit installed harness overrides and an existing local Ollama. Both
harnesses have explicit host networking in this experiment; no egress-isolation claim.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import uuid
from collect_task_evidence import finished
from guard_runtime_smoke import stop_process
from harness_runtime_smoke import rpc
from inference_runtime_smoke import rpc as inference_rpc


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--agentd",default="target/debug/cerynth-agentd")
    parser.add_argument("--inferenced",default="target/debug/cerynth-inferenced")
    parser.add_argument("--model",default="qwen2.5:3b")
    parser.add_argument("--models-directory",help="Explicitly start an owned local Ollama using existing models")
    parser.add_argument("--require-success",action="store_true",help="Fail acceptance if either independently checked artifact is missing")
    parser.add_argument("--simple-copy",action="store_true",help="Use cp for the public copy task; retain native shell approval decisions")
    parser.add_argument("--hermes-call-hint",action="store_true",help="Explicit fixture condition: show the pinned Hermes batch-call shape to the small local model")
    parser.add_argument("--hermes-terminal-profile",action="store_true",help="Owner selects the official eager terminal tool profile for Hermes")
    parser.add_argument("--output",default="artifacts/person3/cross-harness-runtime-smoke.json")
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="cerynth-cross-harness-") as temporary:
        root=Path(temporary);repository=root/"repository";repository.mkdir()
        sponsor_workspace=root/"sponsor";sponsor_workspace.mkdir()
        expected=b"public_fixture=42\n";(repository/"facts.txt").write_bytes(expected)
        subprocess.run(["git","init","--quiet",str(repository)],check=True)
        subprocess.run(["git","-C",str(repository),"add","facts.txt"],check=True)
        subprocess.run(["git","-C",str(repository),"-c","core.hooksPath=/dev/null","-c","user.name=Fixture",
            "-c","user.email=fixture@example.invalid","commit","--quiet","-m","public fixture"],check=True)
        state,inference=root/"agent",root/"inference"
        with socket.socket() as reserved: reserved.bind(("127.0.0.1",0));port=reserved.getsockname()[1]
        coordinator=subprocess.Popen([str(Path(args.inferenced).resolve()),"--directory",str(inference),"serve",
            "--concurrency","1","--listen",f"127.0.0.1:{port}"],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        daemon=None
        try:
            deadline=time.monotonic()+8
            while not (inference/"control.sock").exists():
                assert coordinator.poll() is None and time.monotonic()<deadline;time.sleep(.02)
            if args.models_directory:
                inference_rpc(inference,True,{"type":"start_backend","spec":{"id":"local","binary":"/usr/local/bin/ollama",
                    "models_directory":str(Path(args.models_directory).resolve()),"endpoint":"http://127.0.0.1:11434","context_length":16384,"parallel_requests":1}})
            else:
                inference_rpc(inference,True,{"type":"configure","endpoint":{"kind":"ollama","id":"local","url":"http://127.0.0.1:11434"}})
            environment={**os.environ,"CERYNTH_INFERENCE_DIR":str(inference),"CERYNTH_INFERENCE_HTTP":f"http://127.0.0.1:{port}"}
            daemon=subprocess.Popen([str(Path(args.agentd).resolve()),"--directory",str(state),"serve"],
                env=environment,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            deadline=time.monotonic()+8
            while not (state/"agentd.sock").exists():
                assert daemon.poll() is None and time.monotonic()<deadline;time.sleep(.02)
            parent=rpc(state,{"type":"create","spec":{"name":"routing-sponsor","harness":"generic_pty",
                "workspace":str(sponsor_workspace),"model":{"name":"none"},"runtime":{"project":"public-routing",
                "network":"host","capabilities":["task.delegate"],"budget":{"timeout_seconds":180,"max_subagents":2,
                    "memory_max_bytes":6*1024**3,"memory_high_bytes":4*1024**3,"inference_concurrency":4}}}})
            routed=[]
            for harness in ["open_code","hermes"]:
                child=rpc(state,{"type":"delegate","parent":parent["id"],"spec":{"name":"routed-"+harness,"harness":harness,
                    "workspace":str(repository),"model":{"name":args.model,"endpoint_id":"local","disable_thinking":True},
                    "runtime":{"network":"host","hermes_terminal_profile":harness=="hermes" and args.hermes_terminal_profile,"budget":{"timeout_seconds":120,"max_subagents":0}}},
                    "task":{"id":str(uuid.uuid4()),"objective":"Copy the public fixture using a real tool",
                        "completion_criteria":"result.txt bytes match immutable public fixture","expected_output":"result.txt","dependencies":[]}})
                worktree=Path(rpc(state,{"type":"worktree","id":child["id"]})["path"])
                routed.append((harness,child,worktree))
            rpc(state,{"type":"start","id":parent["id"],"operation_id":str(uuid.uuid4()),"launch":{
                "program":"/usr/bin/sleep","args":["170"],"initial_input":None,"pty":False}})
            def execute(route):
                harness,child,workspace=route
                command="/usr/bin/cp -- facts.txt result.txt" if args.simple_copy else "/usr/bin/python3 -c \"from pathlib import Path; Path('result.txt').write_text(Path('facts.txt').read_text())\""
                task=(f"Your workspace is {workspace}. Use the terminal/bash tool to execute this exact command once: "
                    +command+". "
                    "This is a local fixture, not a network task. Do not modify facts.txt. Execute the command using your tool, then report done.")
                if harness=="hermes" and args.hermes_call_hint:
                    task += " Hermes tool_call expects this argument structure: " + json.dumps({"calls":[{"name":"terminal","arguments":{
                        "command":"/usr/bin/python3 -c \"from pathlib import Path; Path('result.txt').write_text(Path('facts.txt').read_text())\"",
                        "workdir":str(workspace)}}]}) + ". Invoke the available tool_call with that object; do not put command at its top level."
                rpc(state,{"type":"harness_start","id":child["id"],"operation_id":str(uuid.uuid4()),"input":task})
                result=finished(state,child["id"],timeout=140)
                artifact=None
                try: artifact=rpc(state,{"type":"collect_artifact","id":child["id"],"path":"result.txt","kind":"task_result"})
                except RuntimeError: pass
                matched=artifact is not None and artifact["sha256"]==hashlib.sha256(expected).hexdigest()
                return {"harness":harness,"session_id":child["id"],"parent_id":parent["id"],"state":result["state"],
                    "boundary":result["boundary"],"exit_code":result["execution"]["exit_code"],"fixture_verified":matched,
                    "tool_events":sum(e["kind"].startswith("harness_tool") for e in result["execution"]["harness_events"]),
                    "isolated_worktree":True,"artifact":artifact,
                    "synthetic_task_output":rpc(state,{"type":"output","id":child["id"],"offset":0,"limit":32768})["text"]}
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool: trials=list(pool.map(execute,routed))
            graph=rpc(state,{"type":"graph","project":"public-routing"})
            assert len([e for e in graph["edges"] if e["kind"]=="delegation"])==2
            rpc(state,{"type":"stop","id":parent["id"]});finished(state,parent["id"])
            health=inference_rpc(inference,True,{"type":"health"});assert health["grants"]==0
            assert (repository/"facts.txt").read_bytes()==expected and not (repository/"result.txt").exists()
            report={"kind":"real-local-cross-harness-delegation-experiment","model":args.model,"trials":trials,
                "both_harness_tasks_verified":all(t["fixture_verified"] for t in trials),"parent_child_graph_verified":True,
                "shared_inference_global_concurrency":1,"source_repository_unchanged":True,"grants_and_processes_cleaned":True,
                "routing":"explicit owner role-to-harness assignment; no autonomous capability expansion",
                "hermes_call_hint":args.hermes_call_hint,
                "hermes_terminal_profile":args.hermes_terminal_profile,
                "simple_copy":args.simple_copy,
                "network":"explicit host networking; no network-isolation claim","cloud_requests":0,
                "metrics":inference_rpc(inference,True,{"type":"metrics"})}
            destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True)
            destination.write_text(json.dumps(report,indent=2)+"\n");print(json.dumps({"both_harness_tasks_verified":report["both_harness_tasks_verified"],"trials":trials},indent=2))
            if args.require_success:assert report["both_harness_tasks_verified"],"one or more real harness artifacts failed independent verification"
        finally:
            if daemon is not None:stop_process(daemon)
            stop_process(coordinator)


if __name__=="__main__":main()
