#!/usr/bin/env python3
"""Actual idle-model eviction, Guard CUDA work, lazy reload and restart fencing.

Compares the experimental owner-actuated CacheWeaver recommendation with Ollama
keep-alive defaults. Only installed models and public synthetic prompts are used.
"""
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import shutil
import socket
import statistics
import subprocess
import tempfile
import time
import uuid

from guard_runtime_smoke import guard_rpc, stop_process
from harness_runtime_smoke import rpc
from inference_runtime_smoke import rpc as inference_rpc


def wait_socket(path, process):
    deadline = time.monotonic() + 12
    while not path.exists():
        assert process.poll() is None and time.monotonic() < deadline, "service readiness failed"
        time.sleep(.02)

def generation(pid):
    try:return (Path('/proc')/str(pid)/'stat').read_text().rsplit(')',1)[1].split()[19]
    except FileNotFoundError:return None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--models-directory', required=True)
    parser.add_argument('--model', default='qwen3:0.6b')
    parser.add_argument('--trials', type=int, default=3)
    parser.add_argument('--output', default='artifacts/person3/model-residency-smoke.json')
    args = parser.parse_args()
    assert 3 <= args.trials <= 10
    binaries = {name: str(Path('target/debug/cerynth-' + name).resolve())
                for name in ['agentd', 'inferenced', 'guardd', 'guard-exec']}
    with tempfile.TemporaryDirectory(prefix='cerynth-residency-') as temporary:
        root = Path(temporary); state = root/'agent'; control = root/'inference'; guard_run = root/'guard'
        with socket.socket() as reserved:
            reserved.bind(('127.0.0.1', 0)); port = reserved.getsockname()[1]
        config = root/'guard.toml'
        config.write_text(f'runtime_dir = {json.dumps(str(guard_run))}\nstate_dir = {json.dumps(str(root/"security"))}\n'
                          f'guard_exec = {json.dumps(binaries["guard-exec"])}\n')
        guard = subprocess.Popen([binaries['guardd'], 'serve', str(config)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        coordinator = subprocess.Popen([binaries['inferenced'], '--directory', str(control), 'serve'],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        daemon = None
        try:
            wait_socket(guard_run/'guard.sock', guard); wait_socket(control/'control.sock', coordinator)
            assert guard_rpc(guard_run/'guard.sock', {'op':'status'})['type'] == 'status'
            inference_rpc(control, True, {'type':'start_backend','spec':{'id':'local','binary':'/usr/local/bin/ollama',
                'models_directory':str(Path(args.models_directory).resolve()),'endpoint':f'http://127.0.0.1:{port}',
                'context_length':4096,'parallel_requests':1}})
            environment = {**os.environ, 'CERYNTH_GUARD_SOCKET':str(guard_run/'guard.sock'),
                'CERYNTH_INFERENCE_DIR':str(control),'CERYNTH_INFERENCE_HTTP':'http://127.0.0.1:11435'}
            daemon = subprocess.Popen([binaries['agentd'],'--directory',str(state),'serve'],env=environment,
                                       stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            wait_socket(state/'agentd.sock',daemon)
            device = next(d for d in rpc(state,{'type':'inventory'})['gpus'] if d['vendor']=='nvidia')

            def grant():
                session = str(uuid.uuid4())
                return inference_rpc(control,True,{'type':'register','grant':{'session_id':session,'endpoint_id':'local',
                    'trust_domain':'public-residency','allowed_models':[args.model],'concurrency_limits':[[session,1]],
                    'max_priority':0,'disable_thinking':True}})

            def infer(credential):
                result = inference_rpc(control,False,{'session_id':credential['session_id'],'token':credential['token'],
                    'model':args.model,'prompt':'Reply only with the number 4.','max_output_tokens':8,'priority':0})
                assert result['response']['message']['content'].strip(), result
                return {'metrics':result['metrics'],'load_duration_ns':result['response'].get('load_duration'),
                        'eval_count':result['response'].get('eval_count')}

            def resident():
                models = inference_rpc(control,True,{'type':'models','endpoint_id':'local'})['running']['models']
                return next((m for m in models if m.get('name')==args.model or m.get('model')==args.model),None)

            def create(index):
                workspace=root/f'gpu-{index}';workspace.mkdir()
                shutil.copyfile(Path(__file__).with_name('cuda_contention.py'),workspace/'cuda.py')
                return rpc(state,{'type':'create','spec':{'name':f'public-cuda-{index}','harness':'generic_pty',
                    'workspace':str(workspace),'model':{'name':'none'},'runtime':{'capabilities':['gpu.execute','model.release'],
                    'guard':{'capabilities':[{'kind':'fs.read','target':'workspace'},{'kind':'fs.read','target':'/sys'},
                        {'kind':'gpu.use','target':'*'}]},'budget':{'gpu_memory_bytes':256*1024**2,'timeout_seconds':20}}}})

            def run(session):
                job=rpc(state,{'type':'gpu_submit','id':session['id'],'device_id':device['id'],
                               'memory_bytes':256*1024**2,'priority':0})
                assert job['state']=='admitted',job
                rpc(state,{'type':'gpu_run','id':session['id'],'job_id':job['id'],'operation_id':str(uuid.uuid4()),
                    'launch':{'program':'/usr/bin/python3','args':['-I','/workspace/cuda.py','--seconds','2'],
                              'initial_input':None,'pty':False}})
                return job

            def finished(session):
                deadline=time.monotonic()+30
                while True:
                    current=rpc(state,{'type':'status','id':session['id']})
                    if current['state'] in ['completed','failed','cancelled'] and not any(current['execution'][key]
                        for key in ['cleanup_pending','gpu_cleanup_pending','model_residency_cleanup_pending']):break
                    assert time.monotonic()<deadline,current;time.sleep(.02)
                output=rpc(state,{'type':'output','id':session['id'],'offset':0,'limit':32768})['text']
                assert current['state']=='completed' and '"allocated_bytes": 67108864' in output,(current,output)
                return current

            credential=grant(); cold=infer(credential); trials=[]
            # Alternate order across repeated pairs to expose reload regressions.
            for trial in range(args.trials):
                for policy in (['engine_default','cerynth_idle_evict'] if trial%2==0 else ['cerynth_idle_evict','engine_default']):
                    infer(credential);before=resident();assert before and before.get('size_vram',0)>0,before
                    session=create(f'{trial}-{policy}');lease=str(uuid.uuid4());start=time.monotonic();plan=None
                    if policy=='cerynth_idle_evict':
                        plan=inference_rpc(control,True,{'type':'plan_cache','entries':[{'id':'idle-model','trust_domain':'public-residency',
                            'model':args.model,'bytes':before['size_vram'],'active_request':False,'priority':0,'phase':'long_tool',
                            'reload_ms':int(cold['metrics']['request_ms']),'recompute_ms':0,'estimated_reuse_after_ms':2000}],
                            'capabilities':{'evict':True},'trust_domain':'public-residency','needed_bytes':64*1024**2})
                        assert plan['decisions'][0]['action']=='evict',plan
                        receipt=rpc(state,{'type':'gpu_quiesce_model','id':session['id'],'lease_id':lease,'endpoint_id':'local','model':args.model})
                        assert receipt['ready'] and receipt['was_loaded'] and resident() is None,receipt
                    prepare_ms=(time.monotonic()-start)*1000
                    run(session)
                    pending=None;blocked=False
                    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                        if policy=='cerynth_idle_evict':
                            pending=pool.submit(infer,credential);time.sleep(.3);blocked=not pending.done();assert blocked
                            try:
                                inference_rpc(control,True,{'type':'release_model','lease_id':lease,'session_id':session['id']})
                                raise AssertionError('live GPU scope released model gate')
                            except RuntimeError as error:assert 'still contains processes' in str(error),str(error)
                        during=resident();end=finished(session)
                        after=pending.result(timeout=40) if pending else infer(credential)
                    trials.append({'trial':trial,'policy':policy,'prepare_ms':prepare_ms,'wall_ms':(time.monotonic()-start)*1000,
                        'model_vram_before_bytes':before['size_vram'],'model_vram_during_gpu_bytes':during.get('size_vram',0) if during else 0,
                        'new_inference_held_until_gpu_cleanup':blocked,'model_gate_released_after_cleanup':not end['execution']['model_residency_cleanup_pending'],
                        'reload':after,'recommendation':plan})
            # Separate crash experiment: lease must survive coordinator death even
            # though per-session credentials are intentionally revoked on restart.
            infer(credential);crash=create('restart');lease=str(uuid.uuid4())
            rpc(state,{'type':'gpu_quiesce_model','id':crash['id'],'lease_id':lease,'endpoint_id':'local','model':args.model})
            units=json.loads((control/'backend-scopes.json').read_text())['units'];assert len(units)==1
            group=subprocess.run(['/usr/bin/systemctl','--user','show',units[0],'--property=ControlGroup','--value'],capture_output=True,text=True,check=True).stdout.strip()
            assert group.startswith('/')
            cgroup=Path('/sys/fs/cgroup')/group.lstrip('/')
            owned={int(pid):generation(int(pid)) for file in cgroup.rglob('cgroup.procs') for pid in file.read_text().split()}
            assert owned
            coordinator.kill();coordinator.wait(timeout=5)
            for name in ['control.sock','inference.sock']:
                path=control/name
                if path.exists():path.unlink()
            coordinator=subprocess.Popen([binaries['inferenced'],'--directory',str(control),'serve'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            wait_socket(control/'control.sock',coordinator)
            assert all(generation(pid)!=identity for pid,identity in owned.items()),'old model scope retained processes after restart'
            leases=inference_rpc(control,True,{'type':'residency'})['leases'];assert len(leases)==1 and leases[0]['lease_id']==lease,leases
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                replacement=grant();pending=pool.submit(infer,replacement)
                time.sleep(.3);assert not pending.done(),'restart silently reopened the residency gate'
                rpc(state,{'type':'stop','id':crash['id']})
                try:pending.result(timeout=10)
                except RuntimeError:pass  # Owned backend was terminated with its coordinator.
                else:raise AssertionError('a previous backend unexpectedly survived verified scope recovery')
            assert not inference_rpc(control,True,{'type':'residency'})['leases']
            # A second crash retains a loaded GPU runner, not just an unloaded
            # server. Restart must reap every process in that exact owned scope.
            inference_rpc(control,True,{'type':'revoke','session_id':replacement['session_id']})
            inference_rpc(control,True,{'type':'start_backend','spec':{'id':'local','binary':'/usr/local/bin/ollama',
                'models_directory':str(Path(args.models_directory).resolve()),'endpoint':f'http://127.0.0.1:{port}',
                'context_length':4096,'parallel_requests':1}})
            loaded_grant=grant();infer(loaded_grant);assert resident()
            unit=json.loads((control/'backend-scopes.json').read_text())['units'][0]
            group=subprocess.run(['/usr/bin/systemctl','--user','show',unit,'--property=ControlGroup','--value'],capture_output=True,text=True,check=True).stdout.strip()
            assert group.startswith('/')
            cgroup=Path('/sys/fs/cgroup')/group.lstrip('/')
            loaded_processes={int(pid):generation(int(pid)) for file in cgroup.rglob('cgroup.procs') for pid in file.read_text().split()}
            assert len(loaded_processes)>=2,'fixture did not include the loaded runner process'
            coordinator.kill();coordinator.wait(timeout=5)
            coordinator=subprocess.Popen([binaries['inferenced'],'--directory',str(control),'serve'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            # Existing socket files are stale until the new process binds them.
            deadline=time.monotonic()+12
            while True:
                try:
                    if inference_rpc(control,True,{'type':'health'})['service_ready']:break
                except (OSError,RuntimeError,ValueError):pass
                assert coordinator.poll() is None and time.monotonic()<deadline;time.sleep(.02)
            assert all(generation(pid)!=identity for pid,identity in loaded_processes.items()),'loaded runner survived restart'
            report={'kind':'real-ollama-guard-cuda-residency-and-policy-comparison','model':args.model,'trials':trials,
                'coordinator_restart_retains_gate':True,'new_grant_after_restart_waits_for_release':True,'owner_stop_releases_unused_lease':True,
                'backend_scope_and_processes_removed_on_restart':True,
                'loaded_model_runner_scope_cleanup_verified':True,
                'live_scope_release_rejected':True,'guard_cuda_all_trials_verified':True,'cloud_requests':0,
                'limitations':['One GPU and small installed model; no general speedup claim','Idle phase explicitly supplied by owner; no prediction',
                    'Engine defaults retain model memory but avoid reload overhead','Policy evicts whole model weights; no KV transfer or LMCache policy actuation',
                    'Reservations are cooperative; arbitrary GPU clients can bypass them','Restart fixture verifies durable gate metadata; backend ownership dies with coordinator']}
            report['policy_summary']={policy:{'n':args.trials,'mean_wall_ms':statistics.mean(t['wall_ms'] for t in trials if t['policy']==policy),
                'mean_during_gpu_model_vram_bytes':statistics.mean(t['model_vram_during_gpu_bytes'] for t in trials if t['policy']==policy)}
                for policy in ['engine_default','cerynth_idle_evict']}
            destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True)
            destination.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report['policy_summary'],indent=2))
        finally:
            if daemon is not None:stop_process(daemon)
            stop_process(coordinator);stop_process(guard)
            journal=control/'backend-scopes.json'
            if journal.exists():
                for unit in json.loads(journal.read_text())['units']:
                    assert unit.startswith('cerynth-model-') and unit.endswith('.scope')
                    uuid.UUID(unit[len('cerynth-model-'):-len('.scope')])
                    subprocess.run(['/usr/bin/systemctl','--user','stop',unit],capture_output=True,timeout=10)


if __name__=='__main__':main()
