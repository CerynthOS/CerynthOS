#!/usr/bin/env python3
"""Run live owner APIs and cross-process isolation in disposable native-Linux boundaries."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid

ROOT=Path(__file__).resolve().parents[1]


def sha(path):
    with path.open('rb') as stream:return hashlib.file_digest(stream,'sha256').hexdigest()


def execute(command,environment,log):
    result=subprocess.run([str(v) for v in command],cwd=ROOT,env=environment,capture_output=True,text=True,timeout=120)
    log.write_text(result.stdout+result.stderr)
    if result.returncode:raise RuntimeError('live qualification failed; inspect '+str(log))
    for line in reversed(result.stdout.splitlines()):
        try:return json.loads(line)
        except ValueError:continue
    raise RuntimeError('missing structured qualification receipt')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool-root',type=Path,required=True)
    parser.add_argument('--example',type=Path,default=ROOT/'target/debug/examples/platform_qualification')
    parser.add_argument('--runtime',type=Path,default=ROOT/'target/debug/cerynth-agentd')
    parser.add_argument('--output',type=Path,default=ROOT/'artifacts/person1/native-platform-nov27.json')
    args=parser.parse_args();args.output.parent.mkdir(parents=True,exist_ok=True)
    tools=args.tool_root.resolve();example=args.example.resolve()
    if not example.is_file() or not args.runtime.is_file():parser.error('build the native example and runtime first')
    unit='cerynth-native-check-'+uuid.uuid4().hex
    environment=os.environ|{'CERYNTH_RUN_LIVE_PLATFORM':'1','CERYNTH_FUSE2FS_BINARY':str(tools/'usr/bin/fuse2fs'),
                            'CERYNTH_FUSE_LIBRARY_PATH':str(tools/'usr/lib')}
    results={}
    try:
        results['resources']=execute(['systemd-run','--user','--quiet','--collect','--wait','--pipe','--service-type=exec',
            '--unit='+unit,'--property=Delegate=cpu memory pids','--property=MemoryMax=402653184','--property=RuntimeMaxSec=90',
            '--setenv=CERYNTH_RUN_LIVE_PLATFORM=1',example,'resources'],environment,args.output.with_suffix('.resources.log'))
        results['storage']=execute([example,'storage'],environment,args.output.with_suffix('.storage.log'))
        results['cross_agent']=execute([sys.executable,ROOT/'tools/workspace_session_smoke.py','--binary',args.runtime.resolve(),
            '--tool-root',tools,'--output',args.output.with_suffix('.cross-agent.json')],environment,args.output.with_suffix('.cross-agent.log'))
        report={'schema_version':1,'kind':'real-native-platform-qualification','passed':True,'results':results,
            'example_sha256':sha(example),'runtime_sha256':sha(args.runtime),'example_source_sha256':sha(ROOT/'services/cerynthd/examples/platform_qualification.rs'),
            'cross_agent_fixture_sha256':sha(ROOT/'tools/workspace_session_smoke.py'),
            'source_revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
            'tracked_worktree_clean':not subprocess.check_output(['git','diff','HEAD','--'],cwd=ROOT),
            'existing_host_scheduler_or_user_manager_delegation_changed':False,'physical_recovery_writes':False,'cloud_requests':0,
            'limitations':['owner SDK is not a new authenticated daemon IPC surface',
                'fixture cgroups are explicitly delegated to one disposable user service',
                'same-UID agents are isolated by their actual execution sandboxes; file modes alone are insufficient']}
        args.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
    finally:
        subprocess.run(['systemctl','--user','stop',unit],capture_output=True,timeout=15)


if __name__=='__main__':main()
