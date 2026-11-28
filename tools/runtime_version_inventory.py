#!/usr/bin/env python3
"""Check pinned local executables and isolated engine metadata without authentication."""
import argparse,hashlib,json,os,re,shutil,subprocess,tempfile
from pathlib import Path

def selected(variable,fallback):
    value=os.environ.get(variable)
    return Path(value if value is not None else fallback)

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--output',default='artifacts/person3/runtime-version-inventory.json');args=parser.parse_args()
    root=Path(__file__).resolve().parents[1];tools=root/'artifacts/person3/tools'
    pinned=[('opencode',selected('CERYNTH_OPENCODE_BINARY',tools/'opencode-1.19.5'),'1.19.5','managed local CLI/HTTP/ACP fixtures'),
        ('hermes',selected('CERYNTH_HERMES_BINARY',tools/'hermes-0.23.1/.venv/bin/hermes'),'0.23.1','managed local CLI and direct terminal profile'),
        ('codex',selected('CERYNTH_CODEX_BINARY',shutil.which('codex') or '/nonexistent'),'0.167.0','local CLI transport; no live cloud test'),
        ('claude',selected('CERYNTH_CLAUDE_BINARY',shutil.which('claude') or '/nonexistent'),'2.1.312','version/recipe only; cloud execution unvalidated'),
        ('ollama',selected('CERYNTH_OLLAMA_BINARY','/usr/local/bin/ollama'),'0.35.0','owned local model inference and residency')]
    executables=[]
    with tempfile.TemporaryDirectory(prefix='cerynth-version-') as temporary:
        for name,binary,expected,coverage in pinned:
            if not binary.is_absolute():raise ValueError('runtime binary selections must be absolute')
            entry={'name':name,'expected':expected,'observed_version':None,
                'explicit_runtime_override_selected':f'CERYNTH_{name.upper()}_BINARY' in os.environ,
                'pin_matches':False,'sha256':None,'coverage':coverage}
            try:
                if not binary.is_file():raise FileNotFoundError('pinned executable missing')
                result=subprocess.run(['/usr/bin/unshare','--user','--map-root-user','--net','--',str(binary),'--version'],env={'PATH':'/usr/local/bin:/usr/bin:/bin','HOME':temporary,
                    'HERMES_HOME':temporary+'/.hermes','OPENCODE_DISABLE_AUTOUPDATE':'true','OPENCODE_DISABLE_MODELS_FETCH':'true',
                    'DO_NOT_TRACK':'1','NO_COLOR':'1'},capture_output=True,text=True,timeout=20)
                versions=re.findall(r'(?<![\d.])\d+\.\d+\.\d+(?![\d.])',result.stdout+'\n'+result.stderr)
                observed=versions[0] if versions else None
                entry.update(observed_version=observed,pin_matches=result.returncode==0 and expected==observed,
                    sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
                if result.returncode:entry['error']='isolated_version_probe_failed'
            except FileNotFoundError:entry['error']='pinned_executable_missing'
            except subprocess.TimeoutExpired:entry['error']='version_probe_timeout'
            except OSError:entry['error']='version_probe_unavailable'
            executables.append(entry)
    engines=[]
    for environment,package,expected in [('vllm','vllm','0.25.0'),('sglang','sglang','0.5.16'),('lmcache','lmcache','0.5.6')]:
        python=root/f'artifacts/person3/engines/{environment}/bin/python'
        entry={'package':package,'expected':expected,'python':None,'version':None,'pin_matches':False,'metadata_only':True,
            'experiment_evidence':'docs/benchmarks/person3/engine-cache-comparison.json'}
        try:
            result=subprocess.run([str(python),'-c',f'import importlib.metadata,json,platform;print(json.dumps({{"python":platform.python_version(),"version":importlib.metadata.version({package!r})}}))'],
                capture_output=True,text=True,timeout=10,check=True)
            value=json.loads(result.stdout);entry.update(**value,pin_matches=value['version']==expected)
        except (OSError,subprocess.SubprocessError,ValueError,KeyError):entry['error']='pinned_engine_metadata_unavailable'
        engines.append(entry)
    report={'kind':'local-pinned-runtime-inventory','executables':executables,'engine_environments':engines,
        'protocol_pins':{'agent_api':'1/v1alpha1 experimental','ACP':1,'MCP':'2025-11-25 initialization subset','A2A':'0.3.0 supervised HTTP subset'},
        'cargo_lock_sha256':hashlib.sha256((root/'Cargo.lock').read_bytes()).hexdigest(),'cloud_requests':0,
        'auth_commands_run':False,'version_probe_network_namespace':'isolated user/net','all_local_pins_match':all(e['pin_matches'] for e in executables+engines),
        'launch_overrides_must_match_probed_selection':True,
        'limitations':['Installed versions and recorded local experiments do not qualify untested provider subscriptions',
            'Protocol subsets are documented; no full MCP/A2A conformance claim','Workspace compiler requirement is Rust 1.99; qualification used Rust 1.99.0']}
    destination=Path(args.output);destination.parent.mkdir(parents=True,exist_ok=True);destination.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2));return 0 if report['all_local_pins_match'] else 1

if __name__=='__main__':raise SystemExit(main())
