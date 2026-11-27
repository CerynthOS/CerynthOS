#!/usr/bin/env python3
"""Boot a signed dm-verity image, then corrupt only its disposable VM overlay."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import shlex
import subprocess
import sys
import tempfile
import time
import uuid
from os_image_vm import ImageVM, ROOT


def sha(path):
    with path.open('rb') as stream: return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool-root', type=Path, required=True)
    parser.add_argument('--image', type=Path, default=ROOT/'artifacts/person1/os-image-verified-root/cerynthos.raw')
    parser.add_argument('--test-key-directory', type=Path, default=ROOT/'artifacts/person1/secure-boot-vm/throwaway-keys')
    parser.add_argument('--key', type=Path, default=ROOT/'artifacts/person1/image-access/id_ed25519')
    parser.add_argument('--output', type=Path, default=ROOT/'artifacts/person1/verified-root-vm.json')
    args=parser.parse_args(); tools=args.tool_root.resolve(); args.output.parent.mkdir(parents=True,exist_ok=True)
    original_hash=sha(args.image)
    build_path=args.image.parent/'cerynth-build.json'
    build=json.loads(build_path.read_text())
    if build.get('image_sha256')!=original_hash or not build.get('verified_root'):
        raise RuntimeError('image must match its verified-root build provenance')
    with tempfile.TemporaryDirectory(prefix='cerynth-verified-root-',dir='/var/tmp') as temporary:
        work=Path(temporary); variables=work/'enrolled.fd'; owner=str(uuid.uuid4())
        env=__import__('os').environ|{'PYTHONPATH':str(tools/'usr/lib/python3.14/site-packages')}
        subprocess.run([sys.executable,str(tools/'usr/bin/virt-fw-vars'),'--input',str(tools/'usr/share/edk2/x64/OVMF_VARS.4m.fd'),
            '--output',str(variables),'--set-pk',owner,str(args.test_key_directory/'PK.crt'),
            '--add-kek',owner,str(args.test_key_directory/'KEK.crt'),'--add-db',owner,str(args.test_key_directory/'candidate.crt'),
            '--no-microsoft','--microsoft-kek','none','--sb'],check=True,env=env,capture_output=True)
        vm=ImageVM(tools,args.image,args.key,work,secure_boot_vars=variables)
        try:
            vm.start()
            secure=vm.ssh("python3 -c \"from pathlib import Path; p=next(Path('/sys/firmware/efi/efivars').glob('SecureBoot-*')); print(p.read_bytes()[4])\"").stdout.strip()
            assert secure=='1','guest firmware Secure Boot is not enabled'
            cmdline=vm.ssh('cat /proc/cmdline').stdout.strip()
            assert re.search(r'\broothash=[0-9a-f]{64}\b',cmdline),cmdline
            table=vm.ssh('dmsetup table').stdout.strip()
            status=vm.ssh('dmsetup status').stdout.strip()
            mounts=vm.ssh('findmnt -rn -o SOURCE,FSTYPE,OPTIONS /').stdout.strip()
            assert ' verity ' in table and re.search(r'\bverity V\b',status),status
            mapped=next(line.split(':',1)[0] for line in table.splitlines() if ' verity ' in line)
            assert re.fullmatch(r'[A-Za-z0-9_.-]{1,64}',mapped)
            readonly=vm.ssh('blockdev --getro '+shlex.quote('/dev/mapper/'+mapped)).stdout.strip()
            assert readonly=='1' and mounts.split()[1]=='overlay',mounts
            vm.ssh('systemctl is-active cerynthd && test -x /usr/lib/cerynth/cerynth-agentd')
            overlay_text=vm.ssh('cat /usr/share/doc/cerynth/image-overlay.json').stdout
            overlay=json.loads(overlay_text)
            assert hashlib.sha256(overlay_text.encode()).hexdigest()==build['prepared_overlay_manifest_sha256']
            assert overlay['source_revision']==build['prepared_overlay_source_revision']
            vm.stop()
            shutil.copyfile(work/'serial.log',args.output.with_suffix('.good.serial.log'))
            (work/'serial.log').write_text('')
            partition=json.loads(subprocess.check_output(['sfdisk','--json',str(args.image)],text=True))['partitiontable']
            root=next(p for p in partition['partitions'] if p.get('type','').lower()=='4f68bce3-e8cd-4db1-96e7-fbcaf984b709')
            offset=root['start']*partition.get('sectorsize',512)+1024
            with args.image.open('rb') as stream: stream.seek(offset); value=stream.read(1)[0]^255
            subprocess.run([str(tools/'usr/bin/qemu-io'),'-f','qcow2','-c',f'write -P {value} {offset} 1',str(work/'guest.qcow2')],
                check=True,capture_output=True,env=vm.env)
            vm.start(wait_for_ssh=False)
            deadline=time.monotonic()+60; corruption=[]
            while time.monotonic()<deadline:
                text=(work/'serial.log').read_text(errors='replace')
                corruption=[line for line in text.splitlines() if re.search(r'verity.*(corrupt|verification failed|hash mismatch)',line,re.I)]
                if corruption: break
                if vm.process.poll() is not None: break
                time.sleep(.1)
            assert corruption,'no actual dm-verity corruption rejection in the kernel log'
            assert vm.ssh('true',check=False,timeout=5).returncode!=0,'corrupted root unexpectedly reached SSH'
            vm.stop(power_cut=True)
            assert sha(args.image)==original_hash,'backing image changed'
            result={'schema_version':1,'kind':'real-signed-verified-root-uefi-vm','passed':True,
                'image_sha256':original_hash,'firmware_secure_boot_enabled':True,'signed_uki_roothash_cmdline':cmdline,
                'device_mapper_table':table,'device_mapper_status':status,'mounts':mounts,
                'verity_device':mapped,'verity_device_read_only':True,
                'read_only_authenticated_lower_root':True,'writable_overlay_ephemeral':True,
                'corruption_in_disposable_overlay_rejected':True,'corruption_evidence':corruption,
                'backing_image_unchanged':True,'host_block_devices_or_firmware_modified':False,
                'test_keys_only':True,'physical_acceptance_claimed':False,
                'build_provenance_sha256':sha(build_path),'payload_source_revision':overlay['source_revision'],
                'guest_payload_manifest_matches_prepared_overlay':True,
                'runner_sha256':sha(Path(__file__)),
                'firmware_sha256':sha(tools/'usr/share/edk2/x64/OVMF_CODE.secboot.4m.fd'),
                'limitations':['development profile with ephemeral root writes; no persistent verified-root A/B migration claim',
                    'test-key signed development payload; not public release acceptance']}
            args.output.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
        finally:
            vm.stop(power_cut=True)
            if (work/'serial.log').exists():shutil.copyfile(work/'serial.log',args.output.with_suffix('.serial.log'))


if __name__=='__main__':main()
