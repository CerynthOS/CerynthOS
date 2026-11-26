#!/usr/bin/env python3
"""Bounded nested CPU contention with owner IPC and native Qt frame observations."""
import argparse,json,os,statistics,subprocess,tempfile,time,uuid
from pathlib import Path
from harness_runtime_smoke import rpc
from guard_runtime_smoke import stop_process
from resource_hierarchy_smoke import wait,admit_when_healthy

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--output',default='artifacts/person3/resource-responsiveness-smoke.json')
    parser.add_argument('--observer',default='artifacts/person3/observer-build/cerynth-observer');args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='cerynth-responsive-') as temporary:
        root=Path(temporary);state=root/'agent'
        daemon=subprocess.Popen([str(Path('target/debug/cerynth-agentd').resolve()),'--directory',str(state),'serve'],
            env={**os.environ,'CERYNTH_RESOURCE_HIERARCHY':'1','CERYNTH_AGENT_MEMORY_MAX_BYTES':str(512*1024**2)},
            stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        view=None
        try:
            wait(lambda:(state/'agentd.sock').exists())
            def create(name,parent=None,cpu=1_000_000):
                workspace=root/name;workspace.mkdir()
                session=rpc(state,{'type':'create','spec':{'name':name,'harness':'generic_pty','workspace':str(workspace),
                    'model':{'name':'none'},'runtime':{'parent_id':parent,'budget':{'memory_max_bytes':256*1024**2 if parent is None else 96*1024**2,
                    'memory_high_bytes':192*1024**2 if parent is None else 64*1024**2,'cpu_quota_micros_per_second':cpu,
                    'inference_concurrency':2 if parent is None else 1,'max_subagents':2,'timeout_seconds':180}}}})
                assert admit_when_healthy(state,session['id'])['decision']=='accept'
                code="import time;print('ready',flush=True);time.sleep(170)" if parent is None else "print('ready',flush=True)\nwhile True: pass"
                rpc(state,{'type':'start','id':session['id'],'operation_id':str(uuid.uuid4()),'launch':{
                    'program':'/usr/bin/python3','args':['-I','-c',code],'initial_input':None,'pty':False}})
                wait(lambda:'ready' in rpc(state,{'type':'output','id':session['id'],'offset':0,'limit':4096})['text']);return session
            parent=create('parent');children=[create('child-'+str(i),parent['id'],500_000) for i in range(2)]
            before=[rpc(state,{'type':'resource_accounting','id':s['id']}) for s in children]
            frames=root/'frames.json'
            view=subprocess.Popen([str(Path(args.observer).resolve()),'--agent-directory',str(state),'--inference-directory',str(root/'unavailable'),
                '--select-session',parent['id'],'--frame-metrics',str(frames),'--exit-after-ms','6000'],
                env={**os.environ,'QT_QPA_PLATFORM':'offscreen','QT_QUICK_BACKEND':'software'},stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
            latencies=[];start=time.monotonic()
            while time.monotonic()-start<6.5:
                tick=time.monotonic();rpc(state,{'type':'status','id':parent['id']});latencies.append((time.monotonic()-tick)*1000);time.sleep(.03)
            assert view.wait(timeout=10)==0,view.stderr.read().decode()
            after=[rpc(state,{'type':'resource_accounting','id':s['id']}) for s in children]
            def usage(record):
                return record['cpu_stat']['usage_usec']
            consumed=[usage(a)-usage(b) for a,b in zip(after,before)];ratio=max(consumed)/min(consumed)
            assert ratio<1.5 and min(consumed)>1_000_000,consumed
            frame=json.loads(frames.read_text());assert frame['frames']>=100,frame
            percentile=lambda values,p:sorted(values)[max(0,(len(values)*p+99)//100-1)]
            p95=percentile(latencies,95);assert p95<500,p95
            rpc(state,{'type':'stop','id':parent['id']})
            wait(lambda:all(not rpc(state,{'type':'status','id':s['id']})['execution']['resource_allocation']['active'] for s in [parent,*children]))
            report={'kind':'nested-quota-native-ui-and-ipc-contention','logical_cpu_budget':1,'child_cpu_quotas':[.5,.5],
                'child_consumed_microseconds':consumed,'max_min_usage_ratio':ratio,'owner_ipc_samples_ms':latencies,
                'owner_ipc_p50_ms':statistics.median(latencies),'owner_ipc_p95_ms':p95,'native_qt_frame_metrics':frame,
                'native_qt_p95_frame_interval_ms':percentile(frame['frame_intervals_ms'],95),'cascade_cleanup_verified':True,
                'cloud_requests':0,'limitations':['One bounded CPU core; not full-machine saturation',
                    'Qt software offscreen rendering; physical KWin/compositor input latency not measured','Synthetic busy loops; not agent task throughput']}
            destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True);destination.write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps({k:report[k] for k in ['max_min_usage_ratio','owner_ipc_p95_ms','native_qt_p95_frame_interval_ms']},indent=2))
        finally:
            if view is not None:stop_process(view)
            stop_process(daemon)

if __name__=='__main__':main()
