#!/usr/bin/env python3
"""Real managed child snapshots/diffs/checks, including failed and mutating checks."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import time
import uuid
from collect_task_evidence import collect, finished
from harness_runtime_smoke import rpc
from guard_runtime_smoke import stop_process


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--binary",default="target/debug/cerynth-agentd")
    parser.add_argument("--output",default="artifacts/person3/task-evidence-runtime-smoke.json")
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="cerynth-evidence-") as temporary:
        root=Path(temporary);state=root/"state";workspace=root/"child";workspace.mkdir()
        sponsor_workspace=root/"sponsor";sponsor_workspace.mkdir()
        daemon=subprocess.Popen([str(Path(args.binary).resolve()),"--directory",str(state),"serve"],
            stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        try:
            deadline=time.monotonic()+8
            while not (state/"agentd.sock").exists():
                assert daemon.poll() is None and time.monotonic()<deadline
                time.sleep(.02)
            parent=rpc(state,{"type":"create","spec":{"name":"sponsor","harness":"generic_pty",
                "workspace":str(sponsor_workspace),"model":{"name":"none"},"runtime":{"capabilities":["task.delegate"],
                "budget":{"memory_max_bytes":4*1024**3,"inference_concurrency":4}}}})
            child=rpc(state,{"type":"delegate","parent":parent["id"],"spec":{"name":"producer","harness":"generic_pty",
                "workspace":str(workspace),"model":{"name":"none"},"runtime":{"budget":{"max_subagents":0}}},
                "task":{"id":str(uuid.uuid4()),"objective":"public add function fixture",
                    "completion_criteria":"owner assertions on positive and negative addition","expected_output":"candidate.py","dependencies":[]}})
            rpc(state,{"type":"start","id":child["id"],"operation_id":str(uuid.uuid4()),"launch":{
                "program":"/usr/bin/python3","args":["-I","-c","from pathlib import Path; Path('candidate.py').write_text('def add(a,b): return a+b\\n'); print('public producer output')"],
                "initial_input":None,"pty":False}})
            assert finished(state,child["id"])["state"]=="completed"
            baseline=root/"baseline.py";baseline.write_text("def add(a,b): return a-b\n")
            # The producer argument above has a literal escaped newline only
            # inside its source string; the executed code writes a real newline.
            launch={"program":"/usr/bin/python3","args":["-I","-c",
                "import importlib.util; s=importlib.util.spec_from_file_location('candidate','candidate.py');m=importlib.util.module_from_spec(s);s.loader.exec_module(m);assert m.add(2,3)==5 and m.add(-1,1)==0;print('two assertions passed')"],
                "initial_input":None,"pty":False}
            passed=collect(state,child["id"],"candidate.py",baseline,launch)
            assert passed["result"]["assertions_command_succeeded"] and passed["result"]["candidate_unchanged_after_check"]
            assert passed["result"]["source_parent_id"]==parent["id"]
            assert {a["kind"] for a in passed["artifacts"].values()}=={"file","diff","test_result"}
            (workspace/"candidate.py").write_text("def add(a,b): return a-b\n")
            failed=collect(state,child["id"],"candidate.py",baseline,launch)
            assert not failed["result"]["assertions_command_succeeded"]
            modifying={**launch,"args":["-I","-c","from pathlib import Path;Path('candidate.py').write_text('changed by checker')"]}
            changed=collect(state,child["id"],"candidate.py",baseline,modifying)
            assert changed["result"]["assertions_command_succeeded"] and not changed["result"]["candidate_unchanged_after_check"]
            sentinel=root/"host-sentinel";sentinel.write_text("public host fixture")
            (workspace/"candidate.py").unlink();(workspace/"candidate.py").symlink_to(sentinel)
            try: collect(state,child["id"],"candidate.py",baseline,launch);raise AssertionError("symlink evidence accepted")
            except RuntimeError: pass
            report={"kind":"real-managed-child-evidence-smoke","child_task_parent_correlated":True,
                "immutable_candidate_diff_output_and_test_artifacts":True,"independent_checker_boundary":passed["result"]["checker_boundary"],
                "checker_network":"deny","passing_assertions_collected":True,"failed_assertions_preserved":True,
                "checker_mutation_detected":True,"symlink_candidate_denied":True,"agent_claim_trusted":False,
                "general_task_correctness_claim":False,"cloud_requests":0}
            destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True)
            destination.write_text(json.dumps(report,indent=2)+"\n");print(json.dumps(report,indent=2))
        finally: stop_process(daemon)


if __name__=="__main__":main()
