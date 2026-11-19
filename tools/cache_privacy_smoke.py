#!/usr/bin/env python3
"""Strict process-generation cache boundaries on real owned local Ollama servers.

Public canaries only. This checks enforcement and cleanup, not statistical timing
noninterference or the security of an arbitrary external inference engine.
"""
import argparse,json,os,socket,stat,subprocess,tempfile,time,uuid,urllib.error,urllib.request
from pathlib import Path
from inference_runtime_smoke import rpc
from guard_runtime_smoke import stop_process
from resource_hierarchy_smoke import wait

def port():
    with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--models-directory',required=True)
    parser.add_argument('--model',default='qwen3:0.6b');parser.add_argument('--output',default='artifacts/person3/cache-privacy-smoke.json');args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='cerynth-cache-private-') as temporary:
        directory=Path(temporary)/'inference';http_port=port();backend_ports=[port(),port()]
        process=subprocess.Popen([str(Path('target/debug/cerynth-inferenced').resolve()),'--directory',str(directory),'serve','--listen',f'127.0.0.1:{http_port}'],
            env={**os.environ,'CERYNTH_CACHE_PRIVACY':'managed_process'},stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        try:
            wait(lambda:(directory/'control.sock').exists());assert rpc(directory,True,{'type':'health'})['cache_privacy']=='managed_process'
            def start(index):
                rpc(directory,True,{'type':'start_backend','spec':{'id':['a','b'][index],'binary':'/usr/local/bin/ollama',
                    'models_directory':str(Path(args.models_directory).resolve()),'endpoint':f'http://127.0.0.1:{backend_ports[index]}',
                    'context_length':4096,'parallel_requests':1}})
            for i in range(2):start(i)
            def registration(endpoint,sid=None,domain='public-owner'):
                return {'type':'register','grant':{'session_id':sid or str(uuid.uuid4()),'endpoint_id':endpoint,'trust_domain':domain,
                    'allowed_models':[args.model],'concurrency_limits':[[str(uuid.uuid4()),1]],'max_priority':0,'disable_thinking':True}}
            first_registration=registration('a');a=rpc(directory,True,first_registration);b=rpc(directory,True,registration('b'))
            assert a['cache_privacy']==b['cache_privacy']=='managed_process'
            def infer(grant,prompt):
                result=rpc(directory,False,{'session_id':grant['session_id'],'token':grant['token'],'model':args.model,
                    'prompt':prompt,'max_output_tokens':12,'priority':0})
                return result['response']['message']['content']
            canary='PUBLIC-CACHE-CANARY-'+uuid.uuid4().hex
            answer_a=infer(a,'This public test session has private key '+canary+'. Reply only with the number 4.')
            answer_b=infer(b,'You have no prior messages. Reply only with the number 4.')
            assert answer_a.strip() and answer_b.strip() and canary not in answer_b
            def denied(operation,reason):
                try:rpc(directory,True,operation);raise AssertionError('privacy boundary accepted an invalid operation')
                except RuntimeError as error:assert reason in str(error),str(error)
            denied(registration('a'),'another trust domain') # Same owner, different session.
            denied({'type':'configure','endpoint':{'kind':'ollama','id':'alias','url':f'http://127.0.0.1:{backend_ports[0]}'}},'coordinator-owned')
            for mode in ['shared_trust_domain','vllm_per_grant_salt']:
                denied({'type':'configure','endpoint':{'kind':'compatible','id':'external','config':{
                    'url':'http://127.0.0.1:9/v1','cloud_consent':False,'credential_env':None,'cache_isolation':mode}}},'unqualified external KV')
            request=urllib.request.Request(f'http://127.0.0.1:{http_port}/v1/chat/completions',data=json.dumps({
                'model':args.model,'messages':[{'role':'user','content':'public fixture'}],'cache_salt':'forged'}).encode(),
                headers={'Authorization':'Bearer '+a['token'],'Content-Type':'application/json'})
            try:urllib.request.urlopen(request,timeout=10);raise AssertionError('agent selected cache scope')
            except urllib.error.HTTPError as error:assert error.code==400,error.code
            homes=list((directory/'model-homes').iterdir());assert len(homes)==2 and all(stat.S_IMODE(p.stat().st_mode)==0o700 for p in homes)
            before=set(json.loads((directory/'backend-scopes.json').read_text())['units']);assert len(before)==2
            rpc(directory,True,{'type':'revoke','session_id':a['session_id']})
            denied(registration('a',sid=a['session_id'],domain='changed-authorization'),'another trust domain')
            receipt=rpc(directory,True,{'type':'stop_backend','endpoint_id':'a'});assert receipt['private_cache_generation_reset']
            after=set(json.loads((directory/'backend-scopes.json').read_text())['units']);assert len(after)==1 and after<before
            start(0);renewed=rpc(directory,True,registration('a',domain='changed-authorization'))
            assert infer(renewed,'Reply only with the number 4.').strip()
            assert len(list((directory/'model-homes').iterdir()))==3,'private process home was reused across cache generations'
            stop_process(process)
            # Forgetting the environment flag must not silently weaken persisted policy.
            environment={k:v for k,v in os.environ.items() if k!='CERYNTH_CACHE_PRIVACY'}
            process=subprocess.Popen([str(Path('target/debug/cerynth-inferenced').resolve()),'--directory',str(directory),'serve'],
                env=environment,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
            def restarted():
                try:return rpc(directory,True,{'type':'health'})
                except (OSError,ValueError):return None
            assert wait(restarted)['cache_privacy']=='managed_process'
            denied({'type':'configure','endpoint':{'kind':'compatible','id':'external','config':{
                'url':'http://127.0.0.1:9/v1','cloud_consent':False,'credential_env':None}}},'unqualified external KV')
            stop_process(process)
            process=subprocess.Popen([str(Path('target/debug/cerynth-inferenced').resolve()),'--directory',str(directory),'serve'],
                env={**environment,'CERYNTH_CACHE_PRIVACY':'trust_domain'},stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
            assert process.wait(timeout=10)!=0,'persisted strict policy was downgraded'
            assert 'cannot be downgraded' in process.stderr.read().decode()
            report={'kind':'real-owned-ollama-private-cache-generation-smoke','model':args.model,
                'same_owner_sessions_have_separate_processes':True,'private_backend_homes':True,'cross_session_origin_reuse_denied':True,
                'endpoint_alias_denied':True,'unqualified_external_kv_engines_denied':True,'agent_cache_scope_forgery_denied':True,
                'authorization_change_requires_verified_process_reset':True,'reset_uses_new_private_home_and_scope':True,
                'cross_session_public_canary_absent':True,'cloud_requests':0,
                'strict_policy_survives_missing_environment_flag':True,'explicit_in_place_downgrade_rejected':True,
                'limitations':['Supported mode trusts the owner-selected Ollama executable and native in-process cache implementation',
                    'Managed agents must use Guard native inference with host networking denied; observed/unisolated host processes are outside this boundary',
                    'No statistical timing-noninterference or arbitrary-engine safety claim','Separate private processes duplicate model residency; the default shared-trust-domain mode is less restrictive']}
            destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True);destination.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
        finally:stop_process(process)

if __name__=='__main__':main()
