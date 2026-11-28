#!/usr/bin/env python3
"""Repeated fixed-work CPU task with equal and semantic session preferences."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time
import uuid
from harness_runtime_smoke import rpc
from guard_runtime_smoke import stop_process
from resource_hierarchy_smoke import wait,admit_when_healthy


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',default='target/debug/cerynth-agentd')
    parser.add_argument('--repeats',type=int,default=5)
    parser.add_argument('--iterations',type=int,default=4_000_000)
    parser.add_argument('--output',type=Path,default=Path('artifacts/person1/phase-policy-benchmark.json'))
    args=parser.parse_args()
    assert 3<=args.repeats<=20 and 100_000<=args.iterations<=100_000_000
    with tempfile.TemporaryDirectory(prefix='cerynth-phase-') as temporary:
        root=Path(temporary);state=root/'state';cpu=min(os.sched_getaffinity(0))
        daemon=subprocess.Popen([str(Path(args.binary).resolve()),'--directory',str(state),'serve'],
            env=os.environ|{'CERYNTH_RESOURCE_HIERARCHY':'1','CERYNTH_PHASE_SCHEDULING':'1','CERYNTH_AGENT_MEMORY_MAX_BYTES':str(512*1024**2)},
            stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        sessions=[]
        try:
            wait(lambda:(state/'agentd.sock').exists())
            def create(name,parent=None):
                workspace=root/name;workspace.mkdir()
                session=rpc(state,{'type':'create','spec':{'name':name,'harness':'generic_pty','workspace':str(workspace),'model':{'name':'none'},
                    'runtime':{'parent_id':parent,'budget':{'memory_max_bytes':256*1024**2 if parent is None else 96*1024**2,
                    'memory_high_bytes':192*1024**2 if parent is None else 64*1024**2,'cpu_quota_micros_per_second':2_000_000 if parent is None else 1_000_000,
                    'inference_concurrency':2 if parent is None else 1,'cpu_weight':100,'max_subagents':2,'timeout_seconds':180}}}})
                assert admit_when_healthy(state,session['id'])['decision']=='accept';sessions.append(session);return session
            def start(session,code):
                rpc(state,{'type':'start','id':session['id'],'operation_id':str(uuid.uuid4()),'launch':{'program':'/usr/bin/python3',
                    'args':['-I','-c',code],'initial_input':None,'pty':False}})
                wait(lambda:'ready' in rpc(state,{'type':'output','id':session['id'],'offset':0,'limit':4096})['text'])
            parent=create('parent');start(parent,'import time;print("ready",flush=True);time.sleep(170)')
            foreground=create('foreground',parent['id']);background=create('background',parent['id'])
            foreground_code=f'''import os,time,json,pathlib
os.sched_setaffinity(0,{{{cpu}}})
print('ready',flush=True)
while True:
 request=pathlib.Path('request.json')
 if not request.exists():time.sleep(.005);continue
 job=json.loads(request.read_text());request.unlink();start=time.monotonic();cpu_start=time.process_time();total=0
 for value in range({args.iterations}):total=(total+value)%1000000007
 result={{'wall_seconds':time.monotonic()-start,'cpu_seconds':time.process_time()-cpu_start,'answer':total}}
 pathlib.Path(job['output']).write_text(json.dumps(result))
'''
            start(foreground,foreground_code)
            start(background,f'import os;os.sched_setaffinity(0,{{{cpu}}});print("ready",flush=True)\nwhile True: pass')
            for session in [parent,foreground]:rpc(state,{'type':'set_phase','id':session['id'],'phase':'tool'})
            observations=[]
            for repeat in range(args.repeats):
                for condition in (['equal','semantic'] if repeat%2==0 else ['semantic','equal']):
                    rpc(state,{'type':'set_phase','id':background['id'],'phase':'tool' if condition=='equal' else 'batch'})
                    expected=100 if condition=='equal' else 40
                    def applied():
                        observed=[rpc(state,{'type':'resource_accounting','id':session['id']}) for session in [foreground,background]]
                        return observed if [entry['cpu_weight'] for entry in observed]==[100,expected] else None
                    accounting=wait(applied)
                    for entry in accounting:assert entry['cpu_max']=='100000 100000',entry
                    output=f'{repeat}-{condition}.json';workspace=root/'foreground'
                    temporary_request=workspace/'request.tmp';temporary_request.write_text(json.dumps({'output':output}));temporary_request.replace(workspace/'request.json')
                    wait(lambda:(workspace/output).exists(),timeout=60)
                    result=json.loads((workspace/output).read_text());assert result['answer']==(args.iterations*(args.iterations-1)//2)%1000000007
                    observations.append({'repeat':repeat,'condition':condition,'background_weight':expected,**result})
            ratios=[]
            for repeat in range(args.repeats):
                pair={item['condition']:item for item in observations if item['repeat']==repeat}
                ratios.append(pair['semantic']['wall_seconds']/pair['equal']['wall_seconds'])
            report={'schema_version':1,'kind':'repeated-phase-priority-fixed-work-benchmark','repeats':args.repeats,'iterations':args.iterations,
                'runtime_sha256':hashlib.file_digest(Path(args.binary).open('rb'),'sha256').hexdigest(),
                'observations':observations,'paired_semantic_over_equal_wall_time':ratios,'mean_paired_ratio':statistics.mean(ratios),
                'stdev_paired_ratio':statistics.stdev(ratios),'hard_cpu_quotas_unchanged':True,'weights_within_owner_ceiling':True,
                'policy_opt_in':True,'host_scheduler_modified':False,'fixture_cpu':cpu,
                'limitations':['Synthetic fixed-work task; not coding-agent completion','Both fixture processes voluntarily pinned to one CPU for controlled contention',
                    'Normal host applications and background image downloads were not stopped','Linux default scheduling; no sched_ext cgroup guarantee',
                    f'{args.repeats} paired trials are not a broad statistical performance claim']}
            args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps({key:report[key] for key in ['repeats','mean_paired_ratio','stdev_paired_ratio','weights_within_owner_ceiling']}))
        finally:stop_process(daemon)


if __name__=='__main__':main()
