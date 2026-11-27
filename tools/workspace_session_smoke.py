#!/usr/bin/env python3
"""Bounded filesystem sessions, real sandbox denial and volume ancestry checks."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import uuid
from harness_runtime_smoke import rpc
from guard_runtime_smoke import stop_process
from resource_hierarchy_smoke import wait,admit_when_healthy


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',default='target/debug/cerynth-agentd')
    parser.add_argument('--volume-tool',default='tools/cerynth-volume')
    parser.add_argument('--tool-root',type=Path)
    parser.add_argument('--output',type=Path,default=Path('artifacts/person1/workspace-session.json'))
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='cerynth-volume-sessions-') as temporary:
        root=Path(temporary);volumes=root/'volumes';state=root/'runtime';identities=[str(uuid.uuid4()) for _ in range(2)]
        command=['python3',str(Path(args.volume_tool).resolve()),'--directory',str(volumes)]
        if args.tool_root:command+=['--tool-root',str(args.tool_root.resolve())]
        def volume(*arguments):return json.loads(subprocess.check_output(command+list(arguments),text=True))['result']
        volume('init','--capacity-bytes',str(256*1024**2))
        for index,identity in enumerate(identities):
            volume('create','--id',identity,'--image-bytes',str(32*1024**2),'--subtree-bytes-max',str((128 if index==0 else 64)*1024**2),*(['--parent',identities[0]] if index else []))
        mounted=[];daemon=None
        try:
            workspaces=[]
            for identity in identities:workspaces.append(volume('mount',identity)['workspace']);mounted.append(identity)
            daemon=subprocess.Popen([str(Path(args.binary).resolve()),'--directory',str(state),'serve'],
                env=os.environ|{'CERYNTH_RESOURCE_HIERARCHY':'1','CERYNTH_AGENT_MEMORY_MAX_BYTES':str(512*1024**2),'CERYNTH_VOLUME_DIRECTORY':str(volumes)},stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
            wait(lambda:(state/'agentd.sock').exists())
            def create(name,workspace,parent=None,quota=64*1024**2):
                return rpc(state,{'type':'create','spec':{'name':name,'harness':'generic_pty','workspace':str(workspace),'model':{'name':'none'},'runtime':{
                    'parent_id':parent,'budget':{'cpu_quota_micros_per_second':1_000_000 if parent is None else 500_000,
                    'memory_max_bytes':128*1024**2 if parent is None else 64*1024**2,'memory_high_bytes':96*1024**2 if parent is None else 48*1024**2,
                    'max_subagents':2,'inference_concurrency':2 if parent is None else 1,'timeout_seconds':60,'workspace_max_bytes':quota}}}})
            def start(session,code):
                assert admit_when_healthy(state,session['id'])['decision']=='accept'
                return rpc(state,{'type':'start','id':session['id'],'operation_id':str(uuid.uuid4()),'launch':{'program':'/usr/bin/python3','args':['-I','-c',code],'initial_input':None,'pty':False}})
            parent=create('volume-parent',workspaces[0],quota=128*1024**2)
            parent_code=f"""import time,pathlib,json
pathlib.Path('owner-private').write_text('parent-private-data')
print('ready',flush=True)
while not pathlib.Path('probe-now').exists():time.sleep(.02)
try: pathlib.Path({str(Path(workspaces[1])/'child-private')!r}).read_text()
except (FileNotFoundError,PermissionError): pass
else: raise AssertionError('peer workspace read succeeded')
pathlib.Path('peer-denial.json').write_text(json.dumps({{'peer_workspace_hidden':True}}))
time.sleep(40)
"""
            start(parent,parent_code)
            wait(lambda:'ready' in rpc(state,{'type':'output','id':parent['id'],'offset':0,'limit':4096})['text'])
            child=create('volume-child',workspaces[1],parent['id'])
            code=f'''import pathlib,json,errno
assert not pathlib.Path({str(volumes/identities[1]/'workspace.ext4')!r}).exists()
try: pathlib.Path({str(Path(workspaces[0])/'owner-private')!r}).read_text()
except (FileNotFoundError,PermissionError): pass
else: raise AssertionError('parent workspace read succeeded')
pathlib.Path('child-private').write_text('child-private-data')
total=0;full=False
with open('agent-payload','wb',buffering=0) as output:
 for _ in range(64):
  try:total+=output.write(b'x'*1024**2)
  except OSError as error:
   assert error.errno in (errno.ENOSPC,errno.EDQUOT);full=True;break
assert full and total<32*1024**2
print(json.dumps({{'enospc':full,'bytes':total,'backing_image_hidden':True}}),flush=True)
'''
            start(child,code)
            wait(lambda:rpc(state,{'type':'status','id':child['id']})['state'] in ['completed','failed'])
            final=rpc(state,{'type':'status','id':child['id']});assert final['state']=='completed',final
            assert (Path(workspaces[1])/'child-private').read_text()=='child-private-data'
            assert (Path(workspaces[0])/'owner-private').read_text()=='parent-private-data'
            (Path(workspaces[0])/'probe-now').write_text('owner-triggered-peer-probe')
            wait(lambda:(Path(workspaces[0])/'peer-denial.json').exists())
            assert json.loads((Path(workspaces[0])/'peer-denial.json').read_text())['peer_workspace_hidden']
            binding=final['execution']['workspace_volume'];assert binding['volume_id']==identities[1] and binding['parent_volume_id']==identities[0]
            output=rpc(state,{'type':'output','id':child['id'],'offset':0,'limit':4096})['text'];receipt=json.loads(output.strip().splitlines()[-1]);assert receipt['enospc'] and receipt['backing_image_hidden']
            forged=root/'forged';forged.mkdir();(forged/'.cerynth-volume.json').write_text(json.dumps({'capacity_bytes':32*1024**2}))
            attacker=create('forged-volume',forged)
            denied=False
            try:start(attacker,'open("unauthorized","w").write("should not execute")')
            except RuntimeError:denied=True
            assert denied and not (forged/'unauthorized').exists()
            rpc(state,{'type':'stop','id':parent['id']});wait(lambda:not rpc(state,{'type':'status','id':parent['id']})['execution']['resource_allocation']['active'])
            report={'schema_version':1,'kind':'real-bounded-workspace-agent-sessions','managed_child_hit_enospc':True,'two_real_agent_processes_peer_workspace_access_denied_both_directions':True,
                'backing_image_and_owner_journal_hidden_by_sandbox':True,'kernel_mount_and_private_journal_attested':True,
                'session_and_volume_ancestry_match':True,'forged_workspace_metadata_rejected_before_process':True,
                'child_workspace_volume':binding,'receipt':receipt,'cloud_requests':0}
            args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
        finally:
            if daemon is not None:stop_process(daemon)
            for identity in reversed(mounted):volume('unmount',identity)
            for identity in reversed(identities):volume('remove',identity)


if __name__=='__main__':main()
