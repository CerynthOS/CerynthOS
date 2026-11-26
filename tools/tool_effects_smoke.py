#!/usr/bin/env python3
"""Actual Guard Git push through the journalled, sandbox-mounted tool socket.

Only a temporary local bare repository is pushed. No real remote, account or
credential is used. The fixture owner approves that exact synthetic operation.
"""
import argparse,concurrent.futures,json,os,shlex,subprocess,tempfile,time,uuid
from pathlib import Path
from guard_runtime_smoke import guard_rpc,stop_process
from harness_runtime_smoke import rpc
from resource_hierarchy_smoke import wait

def git(path,*arguments):
    return subprocess.run(['git','-C',str(path),*arguments],check=True,capture_output=True,text=True).stdout.strip()

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--output',default='artifacts/person3/tool-effects-smoke.json');args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='ce-') as temporary:
        root=Path(temporary);workspace=root/'work';workspace.mkdir();remote=root/'remote.git';counter=root/'counter'
        git(workspace,'init','--quiet');(workspace/'fixture.txt').write_text('public effect fixture\n')
        git(workspace,'add','fixture.txt');git(workspace,'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','--quiet','-m','fixture')
        commit=git(workspace,'rev-parse','HEAD');subprocess.run(['git','init','--bare','--quiet',str(remote)],check=True)
        hook=remote/'hooks/post-receive';hook.write_text('#!/usr/bin/sh\ncat >/dev/null\necho executed >> '+shlex.quote(str(counter))+'\n');hook.chmod(0o700)
        state=root/'a';guard_dir=root/'g';config=root/'guard.toml'
        config.write_text(f'runtime_dir = {json.dumps(str(guard_dir))}\nstate_dir = {json.dumps(str(root/"security"))}\n'
            f'guard_exec = {json.dumps(str(Path("target/debug/cerynth-guard-exec").resolve()))}\nallow_file_remotes = true\n')
        guard=subprocess.Popen([str(Path('target/debug/cerynth-guardd').resolve()),'serve',str(config)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        process=None
        try:
            wait(lambda:(guard_dir/'guard.sock').exists())
            def launch():
                p=subprocess.Popen([str(Path('target/debug/cerynth-agentd').resolve()),'--directory',str(state),'serve'],
                    env={**os.environ,'CERYNTH_GUARD_SOCKET':str(guard_dir/'guard.sock')},stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
                def ready():
                    assert p.poll() is None,p.stderr.read().decode()
                    try:return rpc(state,{'type':'diagnostics'})
                    except (OSError,ValueError):return None
                wait(ready);return p
            process=launch()
            session=rpc(state,{'type':'create','spec':{'name':'public-tool-effect','harness':'generic_pty','workspace':str(workspace),
                'model':{'name':'none'},'runtime':{'guard':{'remotes':{'origin':str(remote)},'capabilities':[
                    {'kind':'fs.read','target':'workspace'},{'kind':'fs.write','target':'workspace'},
                    {'kind':'git.push','target':'origin:refs/heads/agent/*','mode':'approval'}]},'budget':{'max_subagents':0,'timeout_seconds':60}}}})
            request={'schema':'cerynth.guard/v1alpha1','request_id':str(uuid.uuid4()),'session_id':session['id'],
                'action':{'kind':'git.push','target':'origin:refs/heads/agent/fixture','args':{'commit':commit,'force':False}},
                'context':{'origin':'agent','provenance':['public-offline-fixture']}}
            (workspace/'request.json').write_text(json.dumps(request))
            (workspace/'agent.py').write_text('''import json,socket,time
from pathlib import Path
request={'op':'decide','request':json.loads(Path('/workspace/request.json').read_text())}
results=[]
for _ in range(2):
 with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as s:
  s.connect('/run/agent/guard.sock');s.sendall(json.dumps(request).encode()+b'\\n');results.append(json.loads(s.makefile('rb').readline()))
assert results[0]==results[1]
Path('/workspace/proxy-receipt.json').write_text(json.dumps(results[0]))
print('duplicate socket receipt verified',flush=True)
time.sleep(50)
''')
            rpc(state,{'type':'start','id':session['id'],'operation_id':str(uuid.uuid4()),'launch':{
                'program':'/usr/bin/python3','args':['-I','/workspace/agent.py'],'initial_input':None,'pty':False}})
            wait(lambda:(workspace/'proxy-receipt.json').exists())
            receipt=json.loads((workspace/'proxy-receipt.json').read_text());assert receipt['type']=='decision',receipt
            assert receipt['decision']['decision']=='approval_required',receipt
            pending=guard_rpc(guard_dir/'guard.sock',{'op':'pending_approvals'})['approvals'];assert len(pending)==1,pending
            approval=receipt['decision']['approval']
            approved=guard_rpc(guard_dir/'guard.sock',{'op':'approve','response':{'schema':'cerynth.guard/v1alpha1',
                'approval_id':approval['approval_id'],'digest':approval['digest'],'response':'approve','responder':'public-fixture-owner'}})
            assert approved['type']=='decision' and approved['decision']['decision']=='allow',approved
            result=rpc(state,{'type':'tool_await_approval','id':session['id'],'approval_id':approval['approval_id']})
            assert result['decision']['result'],result
            def replay(_):return rpc(state,{'type':'tool_decide','id':session['id'],'request':request})
            with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:receipts=list(pool.map(replay,range(16)))
            assert all(r==result for r in receipts)
            assert counter.read_text().splitlines()==['executed']
            assert git(remote,'rev-parse','refs/heads/agent/fixture')==commit
            changed=json.loads(json.dumps(request));changed['action']['target']='origin:refs/heads/agent/changed'
            try:rpc(state,{'type':'tool_decide','id':session['id'],'request':changed});raise AssertionError('changed action replayed')
            except RuntimeError as error:assert 'different details' in str(error)
            process.kill();process.wait(timeout=5);process=launch()
            assert replay(0)==result,'known completion was lost on restart'
            assert counter.read_text().splitlines()==['executed']
            record=rpc(state,{'type':'tool_effects','id':session['id']})['operations'][0]
            assert record['automatic_resubmission'] is False
            report={'kind':'actual-guard-broker-operation-deduplication','local_temporary_remote_only':True,
                'sandbox_socket_replaced_with_journal_gateway':True,'duplicate_decide_creates_one_approval':True,
                'owner_approval_bound_to_exact_action':True,'concurrent_duplicate_calls':16,'remote_hook_executions':1,
                'changed_action_rejected':True,'known_completion_replayed_after_crash':True,'cloud_requests':0,
                'unknown_acknowledgement_fault_tests':'Rust journal tests; no automatic replay permitted',
                'scope':'Guard-brokered operations; observed/unisolated arbitrary shell is not covered'}
            destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True);destination.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
        finally:
            if process is not None:stop_process(process)
            stop_process(guard)

if __name__=='__main__':main()
