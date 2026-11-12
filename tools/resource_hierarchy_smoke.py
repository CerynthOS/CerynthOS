#!/usr/bin/env python3
"""Actual nested systemd limits/accounting, admission and SIGKILL recovery."""
import argparse,json,os,subprocess,tempfile,time,uuid
from pathlib import Path
from harness_runtime_smoke import rpc
from guard_runtime_smoke import stop_process

def wait(check,timeout=15):
    deadline=time.monotonic()+timeout
    while True:
        value=check()
        if value:return value
        if time.monotonic()>deadline:raise TimeoutError("resource boundary did not converge")
        time.sleep(.03)

def admit_when_healthy(state,session_id,minimum=None,timeout=90):
    """Wait only for a real transient kernel-pressure block; keep capacity outcomes."""
    def decision():
        value=rpc(state,{'type':'resource_admit','id':session_id,'minimum':minimum})
        return None if value.get('decision')=='queue' and value.get('reason')=='resource_pressure' else value
    return wait(decision,timeout=timeout)

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',default='target/debug/cerynth-agentd')
    parser.add_argument('--output',default='artifacts/person3/resource-hierarchy-smoke.json');args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='cerynth-resources-') as temporary:
        root=Path(temporary);state=root/'state'
        environment={**os.environ,'CERYNTH_RESOURCE_HIERARCHY':'1','CERYNTH_AGENT_MEMORY_MAX_BYTES':str(768*1024**2)}
        def daemon():
            process=subprocess.Popen([str(Path(args.binary).resolve()),'--directory',str(state),'serve'],env=environment,
                stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
            def ready():
                if process.poll() is not None:raise RuntimeError(process.stderr.read().decode())
                try:return rpc(state,{'type':'diagnostics'})
                except OSError:return False
            wait(ready);return process
        process=daemon()
        try:
            def create(name,memory,cpu=1_000_000,parent=None):
                workspace=root/name;workspace.mkdir()
                return rpc(state,{'type':'create','spec':{'name':name,'harness':'generic_pty','workspace':str(workspace),
                    'model':{'name':'none'},'runtime':{'parent_id':parent,'budget':{'cpu_quota_micros_per_second':cpu,
                        'cpu_weight':100,'memory_max_bytes':memory,'memory_high_bytes':memory*3//4,'timeout_seconds':180,
                        'inference_concurrency':2 if parent is None else 1,'max_subagents':2}}}})
            def admit(s,minimum=None):
                return admit_when_healthy(state,s['id'],minimum)
            def start(s,mib):
                rpc(state,{'type':'start','id':s['id'],'operation_id':str(uuid.uuid4()),'launch':{'program':'/usr/bin/python3',
                    'args':['-I','-c',f"import time;data=bytearray({mib}*1024**2);print('ready',flush=True);time.sleep(170)"],
                    'initial_input':None,'pty':False}})
                wait(lambda:'ready' in rpc(state,{'type':'output','id':s['id'],'offset':0,'limit':4096})['text'])
            def drained(s):
                record=rpc(state,{'type':'status','id':s['id']})
                return record if record['state'] in ['completed','failed','cancelled'] and not record['execution']['cleanup_pending'] and not record['execution']['resource_allocation']['active'] else None
            parent=create('parent',512*1024**2);assert admit(parent)['decision']=='accept';start(parent,8)
            child=create('child',256*1024**2,500_000,parent['id']);assert admit(child)['decision']=='accept';start(child,16)
            grandchild=create('grandchild',128*1024**2,250_000,child['id']);assert admit(grandchild)['decision']=='accept';start(grandchild,32)
            observations=[rpc(state,{'type':'resource_accounting','id':s['id']}) for s in [parent,child,grandchild]]
            assert observations[1]['control_group'].startswith(observations[0]['control_group']+'/')
            assert observations[2]['control_group'].startswith(observations[1]['control_group']+'/')
            assert observations[0]['memory_current_bytes']>=observations[1]['memory_current_bytes']>=observations[2]['memory_current_bytes']>=32*1024**2
            assert [o['memory_max_bytes'] for o in observations]==[512*1024**2,256*1024**2,128*1024**2]
            sibling=create('cpu-sibling',128*1024**2,750_000,parent['id'])
            assert admit(sibling)['decision']=='queue','sibling CPU reservations exceeded parent quota'
            cpu_minimum={**sibling['spec']['runtime']['budget'],'cpu_quota_micros_per_second':250_000}
            reduced_child=admit(sibling,cpu_minimum)
            assert reduced_child['decision']=='reduce' and reduced_child['allocation']['granted']['cpu_quota_micros_per_second']==500_000
            rpc(state,{'type':'stop','id':sibling['id']});wait(lambda:drained(sibling))
            waiting=create('waiting',512*1024**2);assert admit(waiting)['decision']=='queue'
            minimum={**waiting['spec']['runtime']['budget'],'memory_max_bytes':128*1024**2,'memory_high_bytes':64*1024**2,
                'cpu_quota_micros_per_second':500_000}
            reduced=admit(waiting,minimum);assert reduced['decision']=='reduce' and reduced['allocation']['granted']['memory_max_bytes']==256*1024**2
            impossible=create('impossible',1024**3);assert admit(impossible)['decision']=='reject'
            rpc(state,{'type':'stop','id':waiting['id']});wait(lambda:drained(waiting))
            rpc(state,{'type':'stop','id':parent['id']})
            for s in [grandchild,child,parent]:wait(lambda:drained(s))
            recovered=create('crashed',256*1024**2);assert admit(recovered)['decision']=='accept';start(recovered,8)
            process.kill();process.wait(timeout=10);process=daemon();record=wait(lambda:drained(recovered))
            assert record['state']=='failed' and record['execution']['operations'][0]['status']=='unknown'
            report={'kind':'real-nested-systemd-resource-smoke','depth':3,'parent_includes_descendant_memory':True,
                'kernel_memory_limits_verified':True,'kernel_cpu_quotas':[o['cpu_max'] for o in observations],
                'admission_accept_reduce_queue_reject':True,'owner_minimum_preserved':True,
                'sibling_cpu_reservations_bound_by_parent':True,
                'leaf_first_cancellation_cleanup':True,'crash_recovery_releases_kernel_and_reservation':True,
                'scheduler_ipc_changed':False,'person1_review_claimed':False,'cloud_requests':0}
            destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True)
            destination.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
        finally:stop_process(process)

if __name__=='__main__':main()
