#!/usr/bin/env python3
"""Real VM updates: interrupted write, bad trial boot, acceptance and rollback."""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import time
from os_image_vm import ImageVM, ROOT
from release_os_image import verify


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool-root', required=True, type=Path)
    parser.add_argument('--image', type=Path, default=ROOT / 'artifacts/person1/os-image-ab/cerynthos.raw')
    parser.add_argument('--bundle', required=True, type=Path)
    parser.add_argument('--private-key', type=Path, default=ROOT / 'artifacts/person1/development-signing/ed25519.pem')
    parser.add_argument('--public-key', type=Path, default=ROOT / 'artifacts/person1/development-signing/ed25519.pub.pem')
    parser.add_argument('--key', type=Path, default=ROOT / 'artifacts/person1/image-access/id_ed25519')
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts/person1/ab-update-qualification.json')
    args = parser.parse_args(); release = verify(args.bundle, args.public_key)
    with tempfile.TemporaryDirectory(prefix='cerynth-ab-vm-', dir='/var/tmp') as temporary:
        work = Path(temporary); vm = ImageVM(args.tool_root, args.image, args.key, work)
        def root_identity():
            return vm.ssh('findmnt -n -o PARTUUID /; cat /proc/sys/kernel/random/boot_id', timeout=5).stdout.splitlines()
        def await_boot(previous, slot, state=None):
            deadline = time.monotonic() + 160
            while time.monotonic() < deadline:
                try:
                    identity = root_identity()
                    if identity[0].endswith('a0' + str(slot)) and identity[1] != previous:
                        if state is None or json.loads(vm.ssh('cat /var/lib/cerynth/updates/pending.json').stdout)['state'] == state:
                            return identity
                except (RuntimeError, subprocess.TimeoutExpired, IndexError): pass
                time.sleep(1)
            raise TimeoutError(f'boot of slot {slot} / {state} not observed')
        def stage(sequence):
            value = dict(release, sequence=sequence)
            metadata = work / 'release.json'; metadata.write_text(json.dumps(value, sort_keys=True, indent=2) + '\n')
            signature = work / 'release.sig'
            subprocess.run(['openssl', 'pkeyutl', '-sign', '-inkey', str(args.private_key), '-rawin',
                            '-in', str(metadata), '-out', str(signature)], check=True, capture_output=True)
            vm.upload(metadata, '/var/lib/cerynth-validation/update/release.json')
            vm.upload(signature, '/var/lib/cerynth-validation/update/release.sig')
            return '/usr/lib/cerynth/cerynth-slot-update stage --directory /var/lib/cerynth-validation/update --public-key /etc/cerynth/update-test.pub'
        try:
            vm.start(); initial = root_identity(); assert initial[0].endswith('a01'), initial
            vm.copy_to(args.bundle, '/var/lib/cerynth-validation/update')
            vm.upload(args.public_key, '/etc/cerynth/update-test.pub')
            vm.ssh("printf 'preserve-me' > /etc/cerynth/operator-config; printf 'persistent-home' > /home/cerynth/update-marker; printf 'persistent-var' > /var/lib/cerynth-validation/update-marker; sync")
            command = stage(release['sequence'])
            first_block = "dd if=/dev/disk/by-partuuid/ad67ad3a-5554-41d8-a025-8b6617848a02 bs=4096 count=1 status=none | sha256sum"
            before = vm.ssh(first_block).stdout
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as executor:
                operation = executor.submit(vm.ssh, command, False, 200)
                deadline = time.monotonic() + 120
                while vm.ssh(first_block).stdout == before:
                    if operation.done(): raise RuntimeError('stage ended before interrupted-write test: ' + operation.result().stderr)
                    if time.monotonic() > deadline: raise TimeoutError('inactive root write did not begin')
                    time.sleep(.1)
                vm.stop(power_cut=True); operation.result(timeout=10)
            vm.start(); interrupted = root_identity(); assert interrupted[0] == initial[0], interrupted
            assert json.loads(vm.ssh('cat /var/lib/cerynth/updates/pending.json').stdout)['state'] == 'writing'
            vm.ssh(stage(release['sequence'] + 1), timeout=200)
            path = '/efi/loader/entries/cerynth-trial-b.conf'
            code = f'from pathlib import Path;p=Path({path!r});p.write_text(p.read_text().replace("rw panic=5", "rw panic=5 init=/bin/false"))'
            vm.ssh('python3 -c ' + shlex.quote(code)); vm.ssh('sync; systemctl reboot', check=False)
            fallback = await_boot(interrupted[1], 1, 'rolled_back')
            vm.ssh(stage(release['sequence'] + 2), timeout=200)
            vm.ssh('systemctl reboot', check=False)
            accepted = await_boot(fallback[1], 2, 'committed')
            assert vm.ssh('cat /etc/cerynth/operator-config; cat /home/cerynth/update-marker; cat /var/lib/cerynth-validation/update-marker').stdout == 'preserve-mepersistent-homepersistent-var'
            vm.ssh('systemctl reboot', check=False)
            stable = await_boot(accepted[1], 2, 'committed')
            vm.ssh('/usr/lib/cerynth/cerynth-slot-update rollback; systemctl reboot', check=False)
            rollback = await_boot(stable[1], 1, 'manual_rollback')
            vm.ssh('systemctl reboot', check=False)
            await_boot(rollback[1], 1, 'manual_rollback')
            report = {'schema_version': 1, 'kind': 'real-uefi-ab-update-and-rollback',
                'image_sha256': hashlib.file_digest(args.image.open('rb'), 'sha256').hexdigest(),
                'signed_rootfs_sha256': release['rootfs']['sha256'], 'test_signer_sha256': release['public_key_sha256'],
                'interrupted_inactive_write_kept_previous_root_bootable': True,
                'unbootable_trial_fell_back_automatically': True, 'healthy_trial_became_persistent_default': True,
                'explicit_configuration_and_home_var_preserved': True, 'manual_rollback_persisted': True,
                'host_block_devices_written': False, 'official_release_approved': False}
            args.output.write_text(json.dumps(report, indent=2) + '\n'); print(json.dumps(report))
        finally:
            try: vm.stop()
            finally:
                if (work / 'serial.log').exists(): shutil.copyfile(work / 'serial.log', args.output.with_suffix('.serial.log'))


if __name__ == '__main__': main()
