#!/usr/bin/env python3
"""Package a signed rootfs/UKI development update from mkosi split artifacts."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
from release_os_image import sign

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image-directory', type=Path, default=ROOT / 'artifacts/person1/os-image-ab')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--private-key', type=Path)
    parser.add_argument('--development-key', action='store_true')
    parser.add_argument('--sequence', type=int, required=True)
    parser.add_argument('--version', required=True)
    args = parser.parse_args()
    key = args.private_key
    if args.development_key:
        if key: parser.error('select one signing identity')
        authority = ROOT / 'artifacts/person1/development-signing'
        authority.mkdir(mode=0o700, parents=True, exist_ok=True); authority.chmod(0o700)
        key = authority / 'ed25519.pem'
        if not key.exists():
            subprocess.run(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(key)], check=True)
            key.chmod(0o600)
            subprocess.run(['openssl', 'pkey', '-in', str(key), '-pubout', '-out', str(authority / 'ed25519.pub.pem')], check=True)
    if not key: parser.error('select an existing signing key or explicit local development identity')
    rootfs = args.image_directory / 'cerynthos.root-a.raw'
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.update-payload-', dir=args.output.parent) as temporary:
        compressed = Path(temporary) / 'rootfs.zst'
        with compressed.open('wb') as output:
            subprocess.run(['zstd', '-q', '-T2', '-3', '-c', str(rootfs)], stdout=output, check=True)
        with rootfs.open('rb') as stream: checksum = hashlib.file_digest(stream, 'sha256').hexdigest()
        result = sign(argparse.Namespace(image=None, rootfs=compressed, uki=args.image_directory / 'cerynthos.efi',
            rootfs_sha256=checksum, rootfs_size=rootfs.stat().st_size, private_key=key, output=args.output,
            inventory=args.image_directory / 'cerynthos.manifest', acceptance=None, sequence=args.sequence, version=args.version))
        print(json.dumps({'output': str(args.output), 'sequence': args.sequence,
                          'signer_sha256': result['public_key_sha256'], 'release_approved': False}))


if __name__ == '__main__': main()
