#!/usr/bin/env python3
"""Install a signed full image into a new file, then boot the installed bytes."""
import argparse
import hashlib
import json
from pathlib import Path
import tempfile
from os_image_vm import ImageVM, ROOT
from release_os_image import install


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool-root', required=True, type=Path)
    parser.add_argument('--bundle', required=True, type=Path)
    parser.add_argument('--public-key', required=True, type=Path)
    parser.add_argument('--key', type=Path, default=ROOT / 'artifacts/person1/image-access/id_ed25519')
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts/person1/installed-image-boot.json')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='cerynth-image-install-', dir='/var/tmp') as temporary:
        root = Path(temporary); target = root / 'installed.raw'
        receipt = install(argparse.Namespace(directory=args.bundle, public_key=args.public_key, target=target,
            journal=root / 'installation.json', erase_all_data=False, expected_serial=None))
        vm = ImageVM(args.tool_root, target, args.key, root / 'vm')
        try:
            vm.start()
            status = vm.ssh('uname -r; systemctl is-active cerynthd; systemctl is-active user@1000').stdout.splitlines()
            assert status[-2:] == ['active', 'active'], status
            report = {'schema_version': 1, 'kind': 'signed-full-image-install-and-uefi-boot',
                'installed_sha256': hashlib.file_digest(target.open('rb'), 'sha256').hexdigest(),
                'installation_readback_verified': receipt['readback_verified'], 'booted_installed_bytes': True,
                'kernel_and_services': status, 'host_block_devices_written': False,
                'physical_installation_qualified': False}
            args.output.write_text(json.dumps(report, indent=2) + '\n'); print(json.dumps(report))
        finally: vm.stop()


if __name__ == '__main__': main()
