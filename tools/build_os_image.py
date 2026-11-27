#!/usr/bin/env python3
"""Build a pinned development disk image using workspace-local mkosi tools."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool-root', type=Path, required=True)
    parser.add_argument('--desktop', action='store_true')
    parser.add_argument('--ab', action='store_true', help='build the explicitly partitioned development A/B variant')
    parser.add_argument('--verified-root', action='store_true', help='signed development image with dm-verity and ephemeral writable root overlay')
    parser.add_argument('--secure-boot-key', type=Path)
    parser.add_argument('--secure-boot-certificate', type=Path)
    parser.add_argument('--force', action='store_true')
    parser.add_argument('--offline', action='store_true', help='require previously populated metadata and authenticated package cache')
    parser.add_argument('--output-directory', type=Path, help='separate destination for an independent build')
    parser.add_argument('--workspace-directory', type=Path, help='separate temporary build root')
    parser.add_argument('--disk-only', action='store_true', help='omit split kernel/initrd/UKI copies')
    args = parser.parse_args()
    if sum([args.desktop, args.ab, args.verified_root]) > 1:
        parser.error('choose one image variant')
    if args.verified_root and (not args.secure_boot_key or not args.secure_boot_certificate):
        parser.error('--verified-root requires an explicitly selected Secure Boot key/certificate')
    if bool(args.secure_boot_key) != bool(args.secure_boot_certificate):
        parser.error('provide both signing key and certificate')
    tools = args.tool_root.resolve()
    site = tools / 'usr/lib/python3.14/site-packages'
    shim = ROOT / 'artifacts/person1/mkosi-python'
    shim.mkdir(parents=True, exist_ok=True)
    # mkosi starts Python again in its filesystem sandbox. Bind the complete
    # pure-Python PE parser through ExtraSearchPaths, without host installation.
    shutil.copyfile('/usr/bin/python', shim / 'mkosi-python')
    (shim / 'mkosi-python').chmod(0o755)
    shutil.copyfile(site / 'pefile.py', shim / 'pefile.py')
    shutil.copytree(site / 'ordlookup', shim / 'ordlookup', dirs_exist_ok=True)
    # The sandbox locates filesystem helpers by PATH; expose only the needed
    # verified executable rather than replacing the host tool search wholesale.
    shutil.copyfile(tools / 'usr/bin/mkfs.fat', shim / 'mkfs.vfat')
    (shim / 'mkfs.vfat').chmod(0o755)
    for name in ['mcopy', 'mmd']:
        shutil.copyfile(tools / 'usr/bin' / name, shim / name)
        (shim / name).chmod(0o755)
    for name in ['sbsign', 'sbverify']:
        if (tools / 'usr/bin' / name).is_file():
            shutil.copyfile(tools / 'usr/bin' / name, shim / name)
            (shim / name).chmod(0o755)
    (shim / 'python3').write_text('#!/bin/sh\nexport PYTHONPATH="$(dirname "$0")"\nexec /usr/bin/python "$@"\n')
    (shim / 'python3').chmod(0o755)
    command = [str(shim / 'mkosi-python'), str(tools / 'usr/bin/mkosi'), '-C', str(ROOT / 'packaging/image'),
               '--extra-search-path=' + str(shim), '--extra-search-path=' + str(tools / 'usr/lib/systemd'),
               '--cache-directory=' + str(ROOT / 'artifacts/person1/image-cache'),
               '--workspace-directory=' + str((args.workspace_directory or ROOT / 'artifacts/person1/image-work').resolve()),
               '--package-cache-dir=' + str(ROOT / 'artifacts/person1/image-package-cache'),
               '--source-date-epoch=1795190400']
    if args.desktop:
        command += ['--profile=desktop']
    if args.ab:
        command += ['--profile=ab']
    if args.verified_root:
        command += ['--profile=verified-root']
    if args.secure_boot_key:
        command += ['--secure-boot=yes', '--secure-boot-auto-enroll=no', '--secure-boot-key=' + str(args.secure_boot_key.resolve()),
                    '--secure-boot-certificate=' + str(args.secure_boot_certificate.resolve())]
    name = 'os-image-verified-root' if args.verified_root else 'os-image-ab' if args.ab else 'os-image-desktop' if args.desktop else 'os-image'
    output = (args.output_directory or ROOT / 'artifacts/person1' / name).resolve()
    command += ['--output-directory=' + str(output)]
    if args.disk_only:
        command += ['--split-artifacts=']
    if args.force:
        command += ['--force']
    if args.offline:
        command += ['--cache-only=always']
    subprocess.run(command + ['build'], check=True, cwd=ROOT, env=os.environ | {'PYTHONPATH': str(site)})
    image = output / 'cerynthos.raw'
    with image.open('rb') as stream:
        image_hash = hashlib.file_digest(stream, 'sha256').hexdigest()
    overlay = ROOT / ('artifacts/person1/os-image-overlay-desktop' if args.desktop else 'artifacts/person1/os-image-overlay') / 'usr/share/doc/cerynth/image-overlay.json'
    configuration = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                     for path in sorted((ROOT / 'packaging/image').rglob('*')) if path.is_file()}
    proof = {'schema_version': 1, 'kind': 'development-disk-build-provenance',
             'source_revision': subprocess.check_output(['git','rev-parse','HEAD'], cwd=ROOT, text=True).strip(),
             'tracked_worktree_clean': not subprocess.check_output(['git','diff','HEAD','--'], cwd=ROOT),
             'builder_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
             'configuration_sha256': configuration, 'prepared_overlay_manifest_sha256': hashlib.sha256(overlay.read_bytes()).hexdigest(),
             'prepared_overlay_source_revision': json.loads(overlay.read_text()).get('source_revision'),
             'image_sha256': image_hash, 'verified_root': args.verified_root,
             'secure_boot_signing_requested': bool(args.secure_boot_key), 'physical_acceptance_claimed': False}
    (output / 'cerynth-build.json').write_text(json.dumps(proof, indent=2) + '\n')
    print(json.dumps({'development_image': True, 'desktop': args.desktop, 'ab': args.ab, 'verified_root': args.verified_root, 'secure_boot': bool(args.secure_boot_key),
                      'output_directory': str(output)}))


if __name__ == '__main__':
    main()
