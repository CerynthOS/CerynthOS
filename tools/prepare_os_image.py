#!/usr/bin/env python3
"""Prepare an explicit development-image overlay without touching the host OS."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[1]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--development-access',action='store_true',help='create a dedicated local VM SSH key; only its public half enters the image')
    parser.add_argument('--ssh-public-key',type=Path)
    parser.add_argument('--desktop',action='store_true')
    parser.add_argument('--observer-build',type=Path,default=ROOT/'artifacts/person3/observer-build')
    args=parser.parse_args()
    artifacts=ROOT/'artifacts/person1';artifacts.mkdir(parents=True,exist_ok=True)
    cargo=shutil.which('cargo') or str(Path.home()/'.cargo/bin/cargo')
    subprocess.run([cargo,'build','--locked','--release','-p','cerynthd','-p','cerynthctl','-p','cerynth-policy','-p','cerynth-scx',
        '-p','cerynth-agentd','-p','cerynth-inferenced','-p','cerynth-resources'],cwd=ROOT,check=True,env=os.environ|{'CARGO_INCREMENTAL':'0'})
    public=args.ssh_public_key
    if args.development_access:
        if public:parser.error('select a supplied key or dedicated development access')
        access=artifacts/'image-access';access.mkdir(mode=0o700,exist_ok=True);os.chmod(access,0o700)
        key=access/'id_ed25519'
        if not key.exists():subprocess.run(['ssh-keygen','-q','-t','ed25519','-f',str(key),'-N','','-C','cerynth-disposable-vm'],check=True)
        os.chmod(key,0o600);public=key.with_suffix('.pub')
    with tempfile.TemporaryDirectory(prefix='.os-image-overlay-',dir=artifacts) as temporary:
        stage=Path(temporary);stage.chmod(0o755)
        subprocess.run(['bash',str(ROOT/'scripts/install-dev-runtime.sh'),'--root',str(stage),'--no-enable'],check=True,capture_output=True)
        command=['bash',str(ROOT/'scripts/install-agent-runtime.sh'),'--root',str(stage),'--profile','release']
        if args.desktop:command+=['--observer-build',str(args.observer_build.resolve())]
        subprocess.run(command,check=True,capture_output=True)
        def write(name,text,mode=0o644):
            path=stage/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(text);path.chmod(mode)
        def copy(source,name,mode=0o644):
            path=stage/name;path.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(source,path);path.chmod(mode)
        def link(name,target):
            path=stage/name;path.parent.mkdir(parents=True,exist_ok=True);path.symlink_to(target)
        write('etc/cerynth/cerynth.toml','default_profile = "balanced"\nadaptation_enabled = false\nadaptation_mode = "off"\nscheduler_backend = "scx"\nscheduler_binary = "/usr/lib/cerynth/cerynth-scx"\nauto_start = false\n')
        write('etc/sysusers.d/cerynth.conf','g cerynth 1000\nu cerynth 1000:1000 "Cerynth workstation" /home/cerynth /bin/bash\n')
        write('etc/tmpfiles.d/cerynth.conf','d /home/cerynth 0700 cerynth cerynth -\nd /home/cerynth/.config 0700 cerynth cerynth -\nd /home/cerynth/.local 0700 cerynth cerynth -\nd /home/cerynth/.local/state 0700 cerynth cerynth -\nd /home/cerynth/Projects 0755 cerynth cerynth -\nd /var/lib/cerynth-validation 0755 cerynth cerynth -\n')
        write('var/lib/systemd/linger/cerynth','')
        write('etc/systemd/system-preset/00-cerynth.preset','enable cerynthd.service\nenable cerynth-boot-marker.service\nenable systemd-networkd.service\n'+('enable sshd.service\n' if public else '')+('enable sddm.service\n' if args.desktop else ''))
        write('etc/systemd/network/20-cerynth-wired.network','[Match]\nName=en* eth*\n\n[Network]\nDHCP=yes\nIPv6AcceptRA=no\n')
        write('etc/systemd/user-preset/00-cerynth.preset','enable cerynth-agentd.service\nenable cerynth-inferenced.service\n')
        copy(ROOT/'packaging/systemd/user@.service.d/20-cerynth-resources.conf','etc/systemd/system/user@.service.d/20-cerynth-resources.conf')
        copy(ROOT/'tools/cerynth-volume','usr/lib/cerynth/cerynth-volume',0o755)
        copy(ROOT/'tools/cerynth-slot-update','usr/lib/cerynth/cerynth-slot-update',0o755)
        copy(ROOT/'tools/release_os_image.py','usr/lib/cerynth/release_os_image.py',0o755)
        copy(ROOT/'packaging/systemd/cerynth-update-health.service','etc/systemd/system/cerynth-update-health.service')
        write('etc/systemd/system-preset/01-cerynth-update.preset','enable cerynth-update-health.service\n')
        write('usr/lib/systemd/user/cerynth-agentd.service.d/20-resources.conf','[Service]\nEnvironment=CERYNTH_RESOURCE_HIERARCHY=1\nEnvironment=CERYNTH_AGENT_MEMORY_MAX_BYTES=1073741824\nEnvironment=CERYNTH_VOLUME_DIRECTORY=%S/cerynth/volumes\n')
        for service in ['cerynthd','cerynth-boot-marker']:
            link(f'etc/systemd/system/multi-user.target.wants/{service}.service',f'/etc/systemd/system/{service}.service')
        for service in ['cerynth-agentd','cerynth-inferenced']:
            link(f'etc/systemd/user/default.target.wants/{service}.service',f'/usr/lib/systemd/user/{service}.service')
        link('etc/systemd/system/default.target','/usr/lib/systemd/system/graphical.target' if args.desktop else '/usr/lib/systemd/system/multi-user.target')
        if args.desktop:
            link('etc/systemd/system/display-manager.service','/usr/lib/systemd/system/sddm.service')
            copy(ROOT/'apps/cerynth-observer/io.cerynth.Observer.desktop','etc/xdg/autostart/io.cerynth.Observer.desktop')
            if args.development_access:
                write('etc/sddm.conf.d/20-cerynth-development.conf','[Autologin]\nUser=cerynth\nSession=plasma.desktop\nRelogin=false\n')
        if public:
            text=public.read_text().strip()
            if '\n' in text or not text.startswith(('ssh-ed25519 ','ssh-rsa ','ecdsa-sha2-')):raise ValueError('select one OpenSSH public key without options')
            write('root/.ssh/authorized_keys',text+'\n',0o600);(stage/'root').chmod(0o700);(stage/'root/.ssh').chmod(0o700)
            write('etc/ssh/sshd_config.d/20-cerynth-development.conf','PermitRootLogin prohibit-password\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n')
            link('etc/systemd/system/multi-user.target.wants/sshd.service','/usr/lib/systemd/system/sshd.service')
            write('etc/cerynth/development-access','Dedicated public-key administration is enabled. This is a development image.\n')
        tests=['advanced_resources_smoke.py','workspace_session_smoke.py','workspace_quota_smoke.py','phase_policy_benchmark.py',
            'harness_runtime_smoke.py','guard_runtime_smoke.py','resource_hierarchy_smoke.py','inference_runtime_smoke.py','cerynth_runtime.py']
        for name in tests:copy(ROOT/'tools'/name,'usr/share/cerynth/validation/'+name,0o755)
        copy(ROOT/'LICENSE','usr/share/licenses/cerynth/LICENSE')
        manifest={'version':1,'development_image':True,'desktop':args.desktop,'ssh_key_installed':public is not None,
            'source_revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
            'scheduler_autostart':False,'payload':{str(path.relative_to(stage)):hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(stage.rglob('*')) if path.is_file() and not path.is_symlink()}}
        write('usr/share/doc/cerynth/image-overlay.json',json.dumps(manifest,indent=2)+'\n')
        destination=artifacts/('os-image-overlay-desktop' if args.desktop else 'os-image-overlay')
        if destination.exists():
            previous=destination.with_name(destination.name+'.previous')
            if previous.exists():shutil.rmtree(previous)
            destination.rename(previous)
        shutil.copytree(stage,destination,symlinks=True)
        print(json.dumps({'overlay':str(destination),'desktop':args.desktop,'development_access':public is not None,'payload_files':len(manifest['payload'])}))


if __name__=='__main__':main()
