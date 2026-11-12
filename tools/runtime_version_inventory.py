#!/usr/bin/env python3
"""Check pinned local executables and isolated engine metadata without authentication."""
import argparse,hashlib,json,re,shutil,subprocess,tempfile
from pathlib import Path

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--output',default='artifacts/person3/runtime-version-inventory.json');args=parser.parse_args()
    root=Path(__file__).resolve().parents[1];tools=root/'artifacts/person3/tools'
    pinned=[('opencode',tools/'opencode-1.19.5','1.19.5','managed local CLI/HTTP/ACP fixtures'),
        ('hermes',tools/'hermes-0.23.1/.venv/bin/hermes','0.23.1','managed local CLI and direct terminal profile'),
        ('codex',Path(shutil.which('codex') or '/nonexistent'),'0.167.0','local CLI transport; no live cloud test'),
        ('claude',Path(shutil.which('claude') or '/nonexistent'),'2.1.312','version/recipe only; cloud execution unvalidated'),
        ('ollama',Path('/usr/local/bin/ollama'),'0.35.0','owned local model inference and residency')]
    executables=[]
    with tempfile.TemporaryDirectory(prefix='cerynth-version-') as temporary:
        for name,binary,expected,coverage in pinned:
            result=subprocess.run(['/usr/bin/unshare','--user','--map-root-user','--net','--',str(binary),'--version'],env={'PATH':'/usr/local/bin:/usr/bin:/bin','HOME':temporary,
                'HERMES_HOME':temporary+'/.hermes','OPENCODE_DISABLE_AUTOUPDATE':'true','OPENCODE_DISABLE_MODELS_FETCH':'true',
                'DO_NOT_TRACK':'1','NO_COLOR':'1'},capture_output=True,text=True,timeout=20)
            versions=re.findall(r'(?<![\d.])\d+\.\d+\.\d+(?![\d.])',result.stdout+'\n'+result.stderr)
            observed=versions[0] if versions else None
            # Preserve the exact versions and digest, never arbitrary CLI text.
            executables.append({'name':name,'expected':expected,'observed_version':observed,
                'pin_matches':expected==observed,'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'coverage':coverage})
    engines=[]
    for environment,package,expected in [('vllm','vllm','0.25.0'),('sglang','sglang','0.5.16'),('lmcache','lmcache','0.5.6')]:
        python=root/f'artifacts/person3/engines/{environment}/bin/python'
        result=subprocess.run([str(python),'-c',f'import importlib.metadata,json,platform;print(json.dumps({{"python":platform.python_version(),"version":importlib.metadata.version({package!r})}}))'],
            capture_output=True,text=True,timeout=10,check=True)
        value=json.loads(result.stdout);engines.append({'package':package,'expected':expected,**value,'pin_matches':value['version']==expected,
            'metadata_only':True,'experiment_evidence':'docs/benchmarks/person3/engine-cache-comparison.json'})
    report={'kind':'local-pinned-runtime-inventory','executables':executables,'engine_environments':engines,
        'protocol_pins':{'agent_api':'1/v1alpha1 experimental','ACP':1,'MCP':'2025-11-25 initialization subset','A2A':'0.3.0 supervised HTTP subset'},
        'cargo_lock_sha256':hashlib.sha256((root/'Cargo.lock').read_bytes()).hexdigest(),'cloud_requests':0,
        'auth_commands_run':False,'version_probe_network_namespace':'isolated user/net','all_local_pins_match':all(e['pin_matches'] for e in executables+engines),
        'limitations':['Installed versions and recorded local experiments do not qualify untested provider subscriptions',
            'Protocol subsets are documented; no full MCP/A2A conformance claim','Rust workspace MSRV 1.85 remains inconsistent with resolved upstream dependencies; host uses Rust 1.99']}
    destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True);destination.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2));assert report['all_local_pins_match']

if __name__=='__main__':main()
