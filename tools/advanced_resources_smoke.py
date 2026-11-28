#!/usr/bin/env python3
"""Real advanced resource API test; unsupported host controllers must fail closed."""
import argparse
import copy
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import time
import uuid
from harness_runtime_smoke import rpc
from guard_runtime_smoke import stop_process
from resource_hierarchy_smoke import wait, admit_when_healthy


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='target/debug/cerynth-agentd')
    parser.add_argument('--output', type=Path, default=Path('artifacts/person1/advanced-resources.json'))
    parser.add_argument('--require-advanced', action='store_true')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='cerynth-advanced-',dir='/var/tmp') as directory:
        root = Path(directory); state = root/'state'
        daemon = subprocess.Popen([str(Path(args.binary).resolve()), '--directory', str(state), 'serve'],
            env=os.environ | {'CERYNTH_RESOURCE_HIERARCHY':'1','CERYNTH_AGENT_MEMORY_MAX_BYTES':str(512*1024**2)},
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            wait(lambda:(state/'agentd.sock').exists())
            def create(name, limits, parent=None):
                workspace=root/name; workspace.mkdir()
                budget={'memory_max_bytes':128*1024**2 if parent is None else 64*1024**2,
                    'memory_high_bytes':96*1024**2 if parent is None else 48*1024**2,
                    'cpu_quota_micros_per_second':500_000 if parent is None else 250_000,
                    'tasks_max':32 if parent is None else 16,'max_subagents':2,'timeout_seconds':90}
                if limits is not None:budget['platform']=limits
                return rpc(state,{'type':'create','spec':{'name':name,'harness':'generic_pty','workspace':str(workspace),'model':{'name':'none'},
                    'runtime':{'parent_id':parent,'budget':budget}}})
            def start(session, code='import time;print("ready",flush=True);time.sleep(65)'):
                assert admit_when_healthy(state,session['id'])['decision']=='accept'
                return rpc(state,{'type':'start','id':session['id'],'operation_id':str(uuid.uuid4()),'launch':{
                    'program':'/usr/bin/python3','args':['-I','-c',code],'initial_input':None,'pty':False}})
            def ready(session):
                return wait(lambda:'ready' in rpc(state,{'type':'output','id':session['id'],'offset':0,'limit':4096})['text'])
            def stop(session):
                rpc(state,{'type':'stop','id':session['id']})
                wait(lambda:not rpc(state,{'type':'status','id':session['id']})['execution']['resource_allocation']['active'])
            parent=create('swap-parent',{'memory_swap_max_bytes':0}); start(parent); ready(parent)
            accounting=rpc(state,{'type':'resource_accounting','id':parent['id']})
            assert accounting['advanced']['memory_swap_max_bytes']==0
            child=create('swap-child',None,parent['id'])
            assert child['spec']['runtime']['budget']['platform']['memory_swap_max_bytes']==0
            start(child); ready(child)
            assert rpc(state,{'type':'resource_accounting','id':child['id']})['advanced']['memory_swap_max_bytes']==0
            denied=False
            try:create('escalated-child',{'memory_swap_max_bytes':4096},parent['id'])
            except RuntimeError:denied=True
            assert denied,'child expanded swap authority'
            stop(parent)
            assert not rpc(state,{'type':'status','id':child['id']})['execution']['resource_allocation']['active']
            own_group=Path('/sys/fs/cgroup')/next(line.split(':',2)[2].strip().lstrip('/') for line in Path('/proc/self/cgroup').read_text().splitlines() if line.startswith('0:'))
            manager=own_group
            while manager != Path('/sys/fs/cgroup') and not manager.name.startswith('user@'):manager=manager.parent
            controllers=set((manager/'cgroup.controllers').read_text().split())
            advanced='cpuset' in controllers and 'io' in controllers
            cpu=min(os.sched_getaffinity(0))
            if advanced:
                affinity=create('affinity',{'allowed_cpus':[cpu],'io_weight':100,'memory_swap_max_bytes':0})
                start(affinity, 'import os,time,json;print(json.dumps({"cpus":sorted(os.sched_getaffinity(0))}),flush=True);print("ready",flush=True);time.sleep(65)');ready(affinity)
                observed=rpc(state,{'type':'resource_accounting','id':affinity['id']})['advanced']
                assert observed['effective_cpus']==[cpu] and observed['allowed_cpus']==[cpu]
                output=rpc(state,{'type':'output','id':affinity['id'],'offset':0,'limit':4096})['text'];assert json.loads(output.splitlines()[0])['cpus']==[cpu]
                stop(affinity)
                device=os.stat(root).st_dev
                sys_device=Path(f'/sys/dev/block/{os.major(device)}:{os.minor(device)}').resolve()
                if (sys_device/'partition').exists():
                    major,minor=map(int,(sys_device.parent/'dev').read_text().strip().split(':'))
                    device=os.makedev(major,minor)
                io=create('throughput',{'io_devices':[{'major':os.major(device),'minor':os.minor(device),'write_bytes_per_second':1024**2}]})
                code='import os,mmap,time,json;buf=mmap.mmap(-1,1024**2);fd=os.open("payload",os.O_WRONLY|os.O_CREAT|os.O_DIRECT,0o600);t=time.monotonic();[os.write(fd,buf) for _ in range(4)];os.fsync(fd);elapsed=time.monotonic()-t;os.close(fd);open("timing.json","w").write(json.dumps({"seconds":elapsed,"bytes":4*1024**2}));print("ready",flush=True)'
                start(io,code)
                wait(lambda:rpc(state,{'type':'status','id':io['id']})['state'] in ['completed','failed'],timeout=60)
                final=rpc(state,{'type':'status','id':io['id']});assert final['state']=='completed',final
                timing=json.loads((root/'throughput/timing.json').read_text());assert timing['seconds']>=2,timing
            else:
                blocked=create('unsupported-affinity',{'allowed_cpus':[cpu]})
                rejected=False
                try:start(blocked)
                except RuntimeError:rejected=True
                assert rejected,'missing controller silently accepted'
                final=rpc(state,{'type':'status','id':blocked['id']})
                assert final['execution']['process'] is None and final['state']=='failed',final
                timing=None
            report={'schema_version':1,'kind':'real-advanced-platform-resource-api','swap_limit_and_child_inheritance':True,
                'child_authority_expansion_denied':True,'cascade_cleanup':True,'delegated_controllers':sorted(controllers),
                'affinity_and_io_qualified':advanced,'missing_controller_fails_before_process':not advanced,
                'direct_io_timing':timing,'host_policy_modified':False}
            args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
            if args.require_advanced and not advanced:raise SystemExit('cpuset/io delegation required for full advanced qualification')
        finally:stop_process(daemon)


if __name__=='__main__':main()
