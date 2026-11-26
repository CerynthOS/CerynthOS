#!/usr/bin/env python3
"""Live native Qt consumer against real owner APIs and a public synthetic backend."""
import argparse,json,os,socket,subprocess,tempfile,threading,time,uuid
from pathlib import Path
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from harness_runtime_smoke import rpc
from inference_runtime_smoke import rpc as inference_rpc
from guard_runtime_smoke import stop_process
from resource_hierarchy_smoke import admit_when_healthy

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--observer',default='artifacts/person3/observer-build/cerynth-observer')
    parser.add_argument('--agentd',default='target/debug/cerynth-agentd');parser.add_argument('--inferenced',default='target/debug/cerynth-inferenced')
    parser.add_argument('--output',default='artifacts/person3/observer-runtime-smoke.json');args=parser.parse_args()
    class Backend(BaseHTTPRequestHandler):
        def do_POST(self):
            self.rfile.read(int(self.headers['Content-Length']))
            body=json.dumps({'model':'public-model','message':{'role':'assistant','content':'4'},'done':True,
                'eval_count':1,'prompt_eval_count':4,'total_duration':1000000,'eval_duration':500000}).encode()+b'\n'
            self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
        def log_message(self,*_):pass
    backend=ThreadingHTTPServer(('127.0.0.1',0),Backend);thread=threading.Thread(target=backend.serve_forever,daemon=True);thread.start()
    destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='cerynth-observer-') as temporary:
        root=Path(temporary);state=root/'agent';inference=root/'inference';workspace=root/'workspace';workspace.mkdir()
        coordinator=subprocess.Popen([str(Path(args.inferenced).resolve()),'--directory',str(inference),'serve'],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        daemon=None
        try:
            deadline=time.monotonic()+8
            while not (inference/'control.sock').exists():
                assert coordinator.poll() is None and time.monotonic()<deadline;time.sleep(.02)
            inference_rpc(inference,True,{'type':'configure','endpoint':{'kind':'ollama','id':'local','url':f'http://127.0.0.1:{backend.server_port}'}})
            environment={**os.environ,'CERYNTH_RESOURCE_HIERARCHY':'1','CERYNTH_INFERENCE_DIR':str(inference),
                'CERYNTH_INFERENCE_HTTP':'http://127.0.0.1:11435'}
            daemon=subprocess.Popen([str(Path(args.agentd).resolve()),'--directory',str(state),'serve'],env=environment,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
            deadline=time.monotonic()+8
            while not (state/'agentd.sock').exists():
                assert daemon.poll() is None and time.monotonic()<deadline;time.sleep(.02)
            session=rpc(state,{'type':'create','spec':{'name':'Public fixture <b>plain text</b>','harness':'generic_pty','workspace':str(workspace),
                'model':{'name':'none'},'runtime':{'capabilities':['gpu.execute'],'budget':{'memory_max_bytes':256*1024**2,
                'memory_high_bytes':192*1024**2,'gpu_memory_bytes':7*1024**3,'cpu_quota_micros_per_second':500000,'max_subagents':0,'timeout_seconds':30}}}})
            assert admit_when_healthy(state,session['id'])['decision']=='accept'
            code="import json,time;print(json.dumps({'type':'tool_use','id':'public-tool','part':{'state':{'status':'completed'}}}),flush=True);time.sleep(20)"
            rpc(state,{'type':'start','id':session['id'],'operation_id':str(uuid.uuid4()),'launch':{'program':'/usr/bin/python3','args':['-I','-c',code],'initial_input':None,'pty':False}})
            inventory=rpc(state,{'type':'inventory'});device=next(d for d in inventory['gpus'] if d['vendor']=='nvidia')
            for memory in [6*1024**3,1024**3]:
                rpc(state,{'type':'gpu_submit','id':session['id'],'device_id':device['id'],'memory_bytes':memory,'priority':0})
            grant=inference_rpc(inference,True,{'type':'register','grant':{'session_id':session['id'],'endpoint_id':'local','trust_domain':'public-observer',
                'allowed_models':['public-model'],'concurrency_limits':[['session:'+session['id'],1]],'max_priority':0}})
            inference_rpc(inference,False,{'session_id':session['id'],'token':grant['token'],'model':'public-model','prompt':'public fixture 2+2','max_output_tokens':4,'priority':0})
            snapshot=root/'view.json';capture=destination.with_suffix('.png').resolve()
            result=subprocess.run([str(Path(args.observer).resolve()),'--agent-directory',str(state),'--inference-directory',str(inference),
                '--select-session',session['id'],'--snapshot-json',str(snapshot),'--capture',str(capture),'--exit-after-ms','4500'],
                env={**os.environ,'QT_QPA_PLATFORM':'offscreen','QT_QUICK_BACKEND':'software'},capture_output=True,text=True,timeout=20)
            assert result.returncode==0,result.stderr
            assert 'TypeError' not in result.stderr and 'ReferenceError' not in result.stderr,result.stderr
            view=json.loads(snapshot.read_text());assert not view['stale'] and view['readOnly']
            assert any(s['id']==session['id'] for s in view['sessions']) and view['accounting']['memory_max_bytes']==256*1024**2
            assert any(e['category']=='resource' for e in view['events'])
            assert any(e.get('attributes',{}).get('source')=='harness_report' for e in view['events'])
            assert {job['state'] for job in view['jobs']}=={'admitted','queued'}
            assert view['metrics']['samples'] and view['inference']['admission_pressure']['version']==1
            assert grant['token'] not in snapshot.read_text()
            rpc(state,{'type':'stop','id':session['id']})
            report={'kind':'live-qt-owner-api-consumer-smoke','qt_version':subprocess.run(['pkg-config','--modversion','Qt6Core'],capture_output=True,text=True,check=True).stdout.strip(),
                'session_list_consumed':True,'resource_and_harness_provenance_consumed':True,'nested_kernel_accounting_consumed':True,
                'queued_and_admitted_gpu_jobs_displayed':True,'inference_metrics_and_pressure_consumed':True,'raw_credential_absent':True,
                'text_rendering':'plain text for lower-trust session and event labels','read_only_client':True,'capture':capture.name,
                'backend':'public synthetic; not a model-quality or inference-speed benchmark','cloud_requests':0}
            destination.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
        finally:
            if daemon is not None:stop_process(daemon)
            stop_process(coordinator);backend.shutdown();backend.server_close();thread.join(timeout=5)

if __name__=='__main__':main()
