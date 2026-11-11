#!/usr/bin/env python3
"""Owner-invoked child evidence collection with an independent managed checker.

An exit code is evidence that this exact check ran, not general task correctness.
Baseline and check files are explicit owner inputs, never inferred from agent text.
"""
import argparse
import difflib
import hashlib
import json
import os
from pathlib import Path
import time
import uuid
from harness_runtime_smoke import rpc


def finished(directory, identifier, timeout=30):
    deadline = time.monotonic() + timeout
    while True:
        session = rpc(directory, {"type":"status","id":identifier})
        if session["state"] in ["completed","failed","cancelled"] and not any(session["execution"][key]
                for key in ["cleanup_pending","guard_cleanup_pending","gpu_cleanup_pending","workspace_preparing"]):
            return session
        if time.monotonic() > deadline: raise TimeoutError("checker or owned-boundary cleanup did not finish")
        time.sleep(.025)


def output(directory, identifier):
    chunks = []; offset = 0
    while True:
        response = rpc(directory, {"type":"output","id":identifier,"offset":offset,"limit":32768})
        chunks.append(response["text"])
        if response["next_offset"] == offset: break
        offset = response["next_offset"]
        assert offset <= 1024**2
    return "".join(chunks)


def derived_artifact(directory, source, workspace, kind, data):
    assert len(data) <= 16*1024**2, "derived artifact exceeds limit"
    name = ".cerynth-owner-evidence-" + str(uuid.uuid4())
    path = workspace / name
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    identity = os.fstat(descriptor)
    try:
        with os.fdopen(descriptor, "wb") as stream: stream.write(data); stream.flush(); os.fsync(stream.fileno())
        return rpc(directory, {"type":"collect_artifact","id":source,"path":name,"kind":kind})
    finally:
        current = path.lstat()
        assert (current.st_dev,current.st_ino) == (identity.st_dev,identity.st_ino), "evidence staging path changed"
        path.unlink()


def collect(directory, source, path, baseline, test_launch):
    directory = Path(directory).resolve()
    session = finished(directory, source, timeout=1)
    assert session["execution"]["mode"] == "managed", "evidence requires an owned managed source boundary"
    assert not Path(path).is_absolute()
    workspace = Path(session["execution"]["worktree"] or session["spec"]["workspace"])
    candidate = rpc(directory, {"type":"collect_artifact","id":source,"path":str(path),"kind":"file"})
    # Read the immutable daemon-owned snapshot, never execute/import the candidate on the host.
    stored = directory / "artifacts" / str(uuid.UUID(candidate["id"]))
    content = stored.read_bytes()
    assert hashlib.sha256(content).hexdigest() == candidate["sha256"]
    baseline = Path(baseline)
    assert baseline.stat().st_size <= 16*1024**2
    before = baseline.read_bytes()
    patch = "".join(difflib.unified_diff(before.decode("utf-8",errors="replace").splitlines(keepends=True),
        content.decode("utf-8",errors="replace").splitlines(keepends=True), fromfile="baseline/"+str(path), tofile="candidate/"+str(path)))
    checker = rpc(directory, {"type":"create","spec":{"name":"owner-independent-task-check", "harness":"generic_pty",
        "workspace":str(workspace),"model":{"name":"none"},"runtime":{"project":session["spec"]["runtime"]["project"],
        "budget":{"max_subagents":0,"timeout_seconds":15}}}})
    rpc(directory, {"type":"start","id":checker["id"],"operation_id":str(uuid.uuid4()),"launch":test_launch})
    checked = finished(directory, checker["id"])
    assert checked["execution"]["mode"] == "managed", "checker boundary was not managed"
    after = rpc(directory, {"type":"collect_artifact","id":source,"path":str(path),"kind":"file"})
    result = {"source_session_id":source,"source_parent_id":session["spec"]["runtime"]["parent_id"],
        "source_task":session["spec"]["runtime"]["task"],"candidate_sha256":candidate["sha256"],
        "candidate_unchanged_after_check":after["sha256"]==candidate["sha256"],
        "checker_session_id":checker["id"],"checker_boundary":checked["boundary"],"checker_state":checked["state"],
        "checker_exit_code":checked["execution"]["exit_code"],"checker_network":"deny",
        "launch_sha256":checked["execution"]["operations"][0]["fingerprint"],
        "assertions_command_succeeded":checked["execution"]["exit_code"]==0,
        "task_correctness_claim":"limited to the explicit owner-selected assertions; no general correctness claim"}
    receipts = {"candidate":candidate,
        "diff":derived_artifact(directory,source,workspace,"diff",patch.encode()),
        "output":derived_artifact(directory,source,workspace,"file",output(directory,source).encode()),
        "checker_output":derived_artifact(directory,source,workspace,"file",output(directory,checker["id"]).encode()),
        "test_result":derived_artifact(directory,source,workspace,"test_result",json.dumps(result,indent=2).encode())}
    return {"kind":"independent-managed-task-evidence","result":result,"artifacts":receipts,"agent_completion_claim_used":False}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory",required=True)
    parser.add_argument("--session",required=True)
    parser.add_argument("--path",required=True)
    parser.add_argument("--baseline",required=True)
    parser.add_argument("--test-launch",required=True,help="Owner-selected LaunchRequest JSON file")
    parser.add_argument("--output",required=True)
    args = parser.parse_args()
    launch = json.loads(Path(args.test_launch).read_text())
    result = collect(args.directory,args.session,args.path,args.baseline,launch)
    destination = Path(args.output); destination.parent.mkdir(parents=True,exist_ok=True)
    destination.write_text(json.dumps(result,indent=2)+"\n")
    print(json.dumps({"report":str(destination),"assertions_command_succeeded":result["result"]["assertions_command_succeeded"]}))


if __name__ == "__main__": main()
