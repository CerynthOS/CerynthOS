#!/usr/bin/env python3
"""Native local auth inventories in a private profile and network namespace.

The credential-shaped value is a public nonfunctional canary. No host account,
subscription, environment secret or provider endpoint is used by this fixture.
"""
import argparse,json,os,shlex,subprocess,tempfile,time
from pathlib import Path
from harness_runtime_smoke import rpc
from guard_runtime_smoke import stop_process

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--output',default='artifacts/person3/auth-inventory-smoke.json');args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='cerynth-auth-inventory-') as temporary:
        root=Path(temporary);home=root/'home';home.mkdir();state=root/'agent';data=home/'.local/share';auth=data/'opencode/auth.json';auth.parent.mkdir(parents=True)
        canary='public-nonfunctional-credential-canary';auth.write_text(json.dumps({'public-fixture':{'type':'api','key':canary}}));auth.chmod(0o600)
        environment={'PATH':'/usr/local/bin:/usr/bin:/bin','HOME':str(home),'HERMES_HOME':str(home/'.hermes'),
            'XDG_DATA_HOME':str(data),'XDG_CONFIG_HOME':str(home/'.config'),'XDG_STATE_HOME':str(home/'.local/state'),
            'CERYNTH_CODEX_BINARY':'/nonexistent','CERYNTH_CLAUDE_BINARY':'/nonexistent','HOST_SECRET_CANARY':canary}
        for name,path in [('OPENCODE','artifacts/person3/tools/opencode-1.19.5'),('HERMES','artifacts/person3/tools/hermes-0.23.1/.venv/bin/hermes')]:
            wrapper=root/name.lower();wrapper.write_text('#!/usr/bin/bash\nexec /usr/bin/unshare --user --map-root-user --net -- '+shlex.quote(str(Path(path).absolute()))+' "$@"\n');wrapper.chmod(0o700)
            environment['CERYNTH_'+name+'_BINARY']=str(wrapper)
        process=subprocess.Popen([str(Path('target/debug/cerynth-agentd').resolve()),'--directory',str(state),'serve'],env=environment,
            stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        try:
            deadline=time.monotonic()+8
            while not (state/'agentd.sock').exists():
                assert process.poll() is None and time.monotonic()<deadline;time.sleep(.02)
            status=rpc(state,{'type':'adapters'});encoded=json.dumps(status)
            assert canary not in encoded and 'public-fixture' not in encoded,encoded
            by_name={d['name']:d for d in status}
            assert by_name['opencode']['authentication']=='credentials_configured',by_name['opencode']
            assert by_name['hermes']['authentication'] in ['unknown','no_saved_credentials'],by_name['hermes']
            assert all(not by_name[name]['cloud_credentials_inherited'] for name in ['opencode','hermes'])
            report={'kind':'offline-native-auth-inventory','opencode_configured_key_detected':True,
                'hermes_empty_profile_state':by_name['hermes']['authentication'],'provider_and_canary_absent_from_api':True,
                'native_inventory_network_namespace':'unshare user/net; no external network','host_account_used':False,
                'live_provider_key_validated':False,'ambient_environment_credentials_forwarded':False,'cloud_requests':0}
            destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True);destination.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
        finally:stop_process(process)

if __name__=='__main__':main()
