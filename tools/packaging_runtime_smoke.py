#!/usr/bin/env python3
"""Stage actual binaries/units and test isolated runtime-linked user service copies.

Never enables default production units. Test unit names, state directories and
ports are unique; all runtime links and owned state are removed afterward.
"""
import argparse,hashlib,json,os,shutil,socket,subprocess,tempfile,time,uuid
from pathlib import Path
from harness_runtime_smoke import rpc
from inference_runtime_smoke import rpc as inference_rpc
from resource_hierarchy_smoke import wait,admit_when_healthy

def systemctl(*arguments,check=True):
    return subprocess.run(['/usr/bin/systemctl','--user',*arguments],check=check,capture_output=True,text=True,timeout=20)

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--output',default='artifacts/person3/packaging-runtime-smoke.json');args=parser.parse_args()
    unique='cerynth-p3pack-'+uuid.uuid4().hex[:12];units={name:f'{unique}-{name}.service' for name in ['agentd','inferenced']}
    state_home=Path(os.environ.get('XDG_STATE_HOME',str(Path.home()/'.local/state')));owned_state=state_home/unique
    links=Path(f'/run/user/{os.getuid()}/systemd/user')
    with tempfile.TemporaryDirectory(prefix=unique+'-') as temporary:
        root=Path(temporary);stage=root/'image';test_units=root/'units';test_units.mkdir()
        subprocess.run(['bash','scripts/install-agent-runtime.sh','--root',str(stage),'--profile','debug',
            '--observer-build',str(Path('artifacts/person3/observer-build').resolve())],check=True,capture_output=True,text=True)
        manifest={str(p.relative_to(stage)):hashlib.sha256(p.read_bytes()).hexdigest() for p in stage.rglob('*') if p.is_file()}
        executables=['cerynth-agentd','cerynth-inferenced','cerynth-harness-bridge','cerynth-hardware',
            'cerynth-resource-authority','cerynth-worktree-checkout']
        expected={f'usr/lib/cerynth/{name}' for name in executables}|{
            'usr/bin/cerynth-observer','usr/share/applications/io.cerynth.Observer.desktop',
            'usr/share/doc/cerynth/agent-runtime.md','usr/lib/systemd/user/cerynth-agentd.service',
            'usr/lib/systemd/user/cerynth-inferenced.service'}
        assert set(manifest)==expected,manifest
        for name in executables:
            assert manifest['usr/lib/cerynth/'+name]==hashlib.sha256(Path('target/debug',name).read_bytes()).hexdigest()
        authority=root/'authority';policy=root/'authority-policy.json'
        policy.write_text(json.dumps({'version':1,'capacity':{'memory_bytes':128*1024**2,
            'cpu_quota_micros_per_second':1_000_000},'inference_limit':0,'accelerators':{}}))
        helper=str(stage/'usr/lib/cerynth/cerynth-resource-authority')
        subprocess.run([helper,'init','--directory',str(authority),'--policy',str(policy)],check=True)
        assert not json.loads(subprocess.check_output([helper,'status','--directory',str(authority)],text=True))['claims']
        with socket.socket() as reserved:reserved.bind(('127.0.0.1',0));port=reserved.getsockname()[1]
        for name,unit in units.items():
            text=(stage/f'usr/lib/systemd/user/cerynth-{name}.service').read_text()
            text=text.replace('/usr/lib/cerynth/',str(stage/'usr/lib/cerynth')+'/').replace('cerynth/agentd',unique+'/agentd').replace('cerynth/inferenced',unique+'/inferenced')
            text=text.replace('cerynth-inferenced.service',units['inferenced']).replace('127.0.0.1:11435',f'127.0.0.1:{port}')
            if name=='agentd':text=text.replace('UMask=0077','Environment=CERYNTH_RESOURCE_HIERARCHY=1\nEnvironment=CERYNTH_AGENT_MEMORY_MAX_BYTES=536870912\nUMask=0077')
            (test_units/unit).write_text(text)
        try:
            systemctl('link','--runtime',*(str(test_units/unit) for unit in units.values()))
            systemctl('daemon-reload');systemctl('start',units['agentd'])
            agent=owned_state/'agentd';inference=owned_state/'inferenced'
            wait(lambda:(agent/'agentd.sock').exists() and (inference/'control.sock').exists())
            assert inference_rpc(inference,True,{'type':'health'})['service_ready']
            workspace=root/'workspace';workspace.mkdir()
            session=rpc(agent,{'type':'create','spec':{'name':'packaged-startup','harness':'generic_pty','workspace':str(workspace),
                'model':{'name':'none'},'runtime':{'budget':{'memory_max_bytes':128*1024**2,'memory_high_bytes':96*1024**2,
                'cpu_quota_micros_per_second':500_000,'timeout_seconds':30,'max_subagents':0}}}})
            assert admit_when_healthy(agent,session['id'])['decision']=='accept'
            rpc(agent,{'type':'start','id':session['id'],'operation_id':str(uuid.uuid4()),'launch':{
                'program':'/usr/bin/sleep','args':['25'],'initial_input':None,'pty':False}})
            wait(lambda:rpc(agent,{'type':'status','id':session['id']})['execution']['process'] is not None)
            old_pid=systemctl('show',units['agentd'],'--property=MainPID','--value').stdout.strip()
            systemctl('kill','--kill-whom=main','--signal=SIGKILL',units['agentd'])
            wait(lambda:systemctl('show',units['agentd'],'--property=MainPID','--value').stdout.strip() not in ['0',old_pid],timeout=20)
            def recovered():
                try:
                    record=rpc(agent,{'type':'status','id':session['id']})
                    return record if record['state']=='failed' and not record['execution']['resource_allocation']['active'] else None
                except (OSError,RuntimeError):return None
            record=wait(recovered)
            assert record['execution']['operations'][0]['status']=='unknown'
            assert record['execution']['failure_reason']=='daemon_restart'
            assert systemctl('is-active',units['inferenced']).stdout.strip()=='active'
            for name in units:
                properties=systemctl('show',units[name],'--property=NoNewPrivileges','--property=MemoryMax').stdout
                assert 'NoNewPrivileges=yes' in properties
            report={'kind':'staged-user-service-startup-and-restart','staged_sha256':manifest,'staged_native_observer':True,
                'staged_resource_authority_init_status_verified':True,'exact_expected_payload_and_binary_hashes':True,
                'actual_unit_hardening_preserved':True,'isolated_state_and_dynamic_port':True,'restart_on_failure_verified':True,
                'managed_scope_and_nested_slice_recovered':True,'unknown_prior_effects_preserved':True,'inference_dependency_started':True,
                'production_units_enabled':False,'booted_iso':False,'cloud_requests':0,
                'limitations':['Runtime-linked copies use staged executable paths and unique unit/state names',
                    'No distribution image definition exists in upstream checkout; no boot/upgrade/signing qualification']}
            destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True);destination.write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps({k:v for k,v in report.items() if k!='staged_sha256'},indent=2))
        finally:
            systemctl('stop',*units.values(),check=False)
            for unit in units.values():
                path=links/unit
                if path.is_symlink() and path.resolve()==(test_units/unit).resolve():path.unlink()
            systemctl('daemon-reload',check=False);systemctl('reset-failed',*units.values(),check=False)
            if owned_state.exists():shutil.rmtree(owned_state)

if __name__=='__main__':main()
