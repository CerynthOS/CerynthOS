#!/usr/bin/env python3
"""Exercise real Ed25519 verification, tamper rejection and file installation."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
from unittest import mock
import release_os_image


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('artifacts/person1/release-image-smoke.json'))
    args = parser.parse_args()
    script = Path(__file__).with_name('release_os_image.py')
    with tempfile.TemporaryDirectory(prefix='cerynth-release-test-') as temporary:
        root = Path(temporary)
        def run(*command, deny=False):
            result = subprocess.run(command, capture_output=True, text=True, timeout=30)
            assert (result.returncode != 0) if deny else (result.returncode == 0), result.stderr
            return result
        for name in ['signer', 'untrusted']:
            run('openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(root / (name + '.key')))
            run('openssl', 'pkey', '-in', str(root / (name + '.key')), '-pubout', '-out', str(root / (name + '.pub')))
        image = root / 'image.raw'; image.write_bytes(bytes(range(256)) * 8192)
        bundle = root / 'bundle'
        run('python3', str(script), 'sign', '--image', str(image), '--private-key', str(root / 'signer.key'),
            '--output', str(bundle), '--version', '0.1.0-test', '--sequence', '1')
        def verify(key='signer.pub', deny=False):
            return run('python3', str(script), 'verify', '--directory', str(bundle), '--public-key', str(root / key), deny=deny)
        verify(); verify('untrusted.pub', deny=True)
        original=(bundle/'release.json').read_bytes()
        openssl=release_os_image.run
        def replace_after_verification(*arguments):
            result=openssl(*arguments)
            if '-verify' in arguments:
                forged=json.loads(original);forged['version']='substituted-after-verification'
                (bundle/'release.json').write_text(json.dumps(forged))
            return result
        with mock.patch.object(release_os_image,'run',side_effect=replace_after_verification):
            witnessed=release_os_image.verify(bundle,root/'signer.pub')
        assert witnessed['version']=='0.1.0-test','manifest substituted after signature check'
        (bundle/'release.json').write_bytes(original)
        target = root / 'installed.raw'; journal = root / 'install.json'
        command = ['python3', str(script), 'install', '--directory', str(bundle), '--public-key', str(root / 'signer.pub'),
                   '--target', str(target), '--journal', str(journal)]
        run(*command)
        assert target.read_bytes() == image.read_bytes() and json.loads(journal.read_text())['state'] == 'verified'
        run(*command, deny=True)
        target.unlink(); run(*command, deny=True)
        assert not target.exists(), 'replay modified target before authorization'
        with (bundle / 'system.raw').open('r+b') as stream: stream.write(b'corrupted')
        verify(deny=True); run(*command, deny=True)
        assert not target.exists(), 'unverified artifact reached installer'
        (bundle / 'system.raw').unlink(); (bundle / 'system.raw').symlink_to(image)
        verify(deny=True)
        report = {'schema_version': 1, 'kind': 'signed-image-installation-negative-tests',
                  'authentic_bundle_verified': True, 'wrong_trust_key_rejected': True,
                  'tampered_and_symlink_artifacts_rejected': True, 'installed_bytes_verified': True,
                  'journal_replay_rejected_before_target_write': True, 'host_block_devices_written': False}
        report['manifest_substitution_after_signature_verification_prevented']=True
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + '\n'); print(json.dumps(report))


if __name__ == '__main__':
    main()
