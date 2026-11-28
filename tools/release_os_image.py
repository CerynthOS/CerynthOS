#!/usr/bin/env python3
"""Sign, verify and explicitly install development or externally approved public images.

No private key is placed in the bundle. Artifact signatures do not imply UEFI
Secure Boot, release approval, or physical hardware qualification.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tempfile


def run(*arguments):
    return subprocess.check_output(arguments, stderr=subprocess.PIPE)


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def regular(path):
    if not stat.S_ISREG(Path(path).lstat().st_mode):
        raise ValueError('expected a regular file without symlinks: ' + str(path))


def bounded_regular_bytes(path, maximum):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, 'rb') as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode): raise ValueError('expected a regular artifact')
        data = stream.read(maximum + 1)
    if len(data) > maximum: raise ValueError('artifact exceeds its bounded size')
    return data


def unique(pairs):
    result = {}
    for name, value in pairs:
        if name in result:
            raise ValueError('duplicate manifest key')
        result[name] = value
    return result


def verify(directory, public_key, acceptance_authority_public_key=None, require_release=False):
    directory = Path(directory).resolve(strict=True)
    manifest = directory / 'release.json'
    metadata = bounded_regular_bytes(manifest, 1024**2)
    signature = bounded_regular_bytes(directory / 'release.sig', 64)
    if len(signature) != 64: raise ValueError('expected an Ed25519 signature')
    # Verify and parse the same immutable bytes. An unprivileged publisher may
    # replace the source manifest while the administrator verifies a bundle.
    with tempfile.TemporaryDirectory(prefix='cerynth-signature-') as temporary:
        staged = Path(temporary); (staged / 'manifest').write_bytes(metadata); (staged / 'signature').write_bytes(signature)
        run('openssl', 'pkeyutl', '-verify', '-pubin', '-inkey', str(public_key), '-rawin',
            '-in', str(staged / 'manifest'), '-sigfile', str(staged / 'signature'))
    release = json.loads(metadata, object_pairs_hook=unique)
    if release['schema_version'] != 1 or type(release['sequence']) is not int or not 1 <= release['sequence'] <= 2**63 - 1:
        raise ValueError('unsupported release metadata')
    if release.get('channel') not in ['development', 'release']:
        raise ValueError('unsupported artifact channel')
    if require_release and release['channel'] != 'release':
        raise ValueError('a development artifact cannot satisfy a required public release')
    files = release['files']
    if not 1 <= len(files) <= 32 or not ('system.raw' in files or {'rootfs.ext4.zst', 'kernel.efi'} <= files.keys()):
        raise ValueError('incomplete release')
    if release['channel'] == 'release' and (release.get('release_approved') is not True or
            not {'acceptance.json', 'acceptance.sig', 'physical-recovery.json'} <= files.keys() or
            not re.fullmatch('[a-f0-9]{40}', release.get('source_revision', '')) or
            not re.fullmatch('[a-f0-9]{64}', release.get('contract_id', ''))):
        raise ValueError('public release approval metadata is incomplete')
    for name, record in files.items():
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,127}', name):
            raise ValueError('invalid artifact name')
        path = directory / name
        regular(path)
        if path.stat().st_size != record['size'] or digest(path) != record['sha256']:
            raise ValueError('artifact integrity check failed: ' + name)
    if release['channel'] == 'release':
        if acceptance_authority_public_key is None:
            raise ValueError('public artifacts require an independently trusted acceptance authority public key')
        from platform_release_acceptance import candidate, verify_acceptance
        subject = candidate(release, release['source_revision'], release['contract_id'], release.get('qualified_image_sha256'))
        accepted = verify_acceptance(directory / 'acceptance.json', directory / 'acceptance.sig',
            acceptance_authority_public_key, subject, directory / 'physical-recovery.json')
        if any(release.get(name) != value for name, value in accepted.items()):
            raise ValueError('acceptance authority binding differs from the release manifest')
    return release


def sign(args):
    channel = getattr(args, 'channel', 'development')
    if channel not in ['development', 'release']: raise ValueError('unsupported artifact channel')
    if channel == 'release' and not all(getattr(args, name, None) for name in [
            'acceptance', 'acceptance_signature', 'acceptance_authority_public_key', 'physical_recovery', 'source_revision', 'contract_id']):
        raise ValueError('public signing requires an externally signed acceptance record and actual physical recovery evidence')
    if args.image:
        regular(args.image)
    if not args.image and not args.rootfs:
        raise ValueError('supply a disk image or a complete rootfs/UKI update pair')
    if bool(args.rootfs) != bool(args.uki) or (args.rootfs and (not re.fullmatch('[a-f0-9]{64}', args.rootfs_sha256 or '') or not args.rootfs_size or args.rootfs_size <= 0)):
        raise ValueError('rootfs updates require a UKI, uncompressed size and SHA256')
    if args.output.exists():
        raise ValueError('select a new release directory')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.release-', dir=args.output.parent) as temporary:
        stage = Path(temporary)
        if args.image:
            shutil.copyfile(args.image, stage / 'system.raw')
        if args.rootfs:
            for source, name in [(args.rootfs, 'rootfs.ext4.zst'), (args.uki, 'kernel.efi')]:
                regular(source); shutil.copyfile(source, stage / name)
        for source, name in [(args.inventory, 'packages.json'), (args.acceptance, 'acceptance.json')]:
            if source:
                regular(source)
                shutil.copyfile(source, stage / name)
        if channel == 'release':
            for source, name in [(args.acceptance_signature, 'acceptance.sig'), (args.physical_recovery, 'physical-recovery.json')]:
                regular(source); shutil.copyfile(source, stage / name)
        public = run('openssl', 'pkey', '-in', str(args.private_key), '-pubout')
        with tempfile.NamedTemporaryFile() as key:
            key.write(public); key.flush()
            fingerprint = hashlib.sha256(run('openssl', 'pkey', '-pubin', '-in', key.name, '-outform', 'DER')).hexdigest()
        manifest = {'schema_version': 1, 'channel': channel, 'sequence': args.sequence,
                    'version': args.version, 'public_key_sha256': fingerprint,
                    'secure_boot_qualified': False, 'release_approved': False,
                    'files': {path.name: {'size': path.stat().st_size, 'sha256': digest(path)}
                              for path in sorted(stage.iterdir())}}
        if args.rootfs:
            manifest['rootfs'] = {'unpacked_size': args.rootfs_size, 'sha256': args.rootfs_sha256}
        if channel == 'release':
            from platform_release_acceptance import candidate, verify_acceptance
            subject = candidate(manifest, args.source_revision, args.contract_id, getattr(args, 'qualified_image_sha256', None))
            accepted = verify_acceptance(stage / 'acceptance.json', stage / 'acceptance.sig',
                args.acceptance_authority_public_key, subject, stage / 'physical-recovery.json')
            manifest.update(accepted | {'release_approved': True, 'source_revision': args.source_revision, 'contract_id': args.contract_id,
                                        'qualified_image_sha256': subject['qualified_image_sha256']})
        (stage / 'release.json').write_text(json.dumps(manifest, sort_keys=True, indent=2) + '\n')
        run('openssl', 'pkeyutl', '-sign', '-inkey', str(args.private_key), '-rawin',
            '-in', str(stage / 'release.json'), '-out', str(stage / 'release.sig'))
        with tempfile.NamedTemporaryFile() as key:
            key.write(public); key.flush(); verify(stage, Path(key.name), getattr(args, 'acceptance_authority_public_key', None), channel == 'release')
        shutil.copytree(stage, args.output)
    return manifest


def block_target(path, serial):
    if not serial:
        raise ValueError('block installation requires the exact dedicated target serial and --erase-all-data')
    tree = json.loads(run('lsblk', '--json', '--bytes', '--output', 'PATH,SERIAL,SIZE,MOUNTPOINTS,TYPE', str(path)))['blockdevices']
    if len(tree) != 1 or tree[0]['type'] != 'disk' or tree[0]['serial'] != serial:
        raise ValueError('target disk identity does not match the supplied serial')
    def mounted(device):
        return any(device.get('mountpoints') or []) or any(mounted(child) for child in device.get('children', []))
    if mounted(tree[0]):
        raise ValueError('target or a child partition is mounted')
    return tree[0]['size']


def install(args):
    release = verify(args.directory, args.public_key, getattr(args, 'acceptance_authority_public_key', None), getattr(args, 'require_release', False))
    source = args.directory / 'system.raw'
    target = args.target.absolute()
    if target == source.resolve():
        raise ValueError('target is the release artifact')
    mode = target.lstat().st_mode if target.exists() or target.is_symlink() else None
    if mode is not None and stat.S_ISBLK(mode):
        if not args.erase_all_data or block_target(target, args.expected_serial) < source.stat().st_size:
            raise ValueError('target is not explicitly authorized or is too small')
    elif mode is not None:
        raise ValueError('regular-file installation requires a new target file')
    elif args.erase_all_data or args.expected_serial:
        raise ValueError('block-device authorization cannot be reused for a file')
    args.journal.parent.mkdir(parents=True, exist_ok=True)
    lock = os.open(args.journal.with_suffix('.lock'), os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if args.journal.exists():
            previous = json.loads(args.journal.read_text())
            if previous['sequence'] >= release['sequence']:
                raise ValueError('installation journal refuses replay or downgrade')
        def record(state):
            with tempfile.NamedTemporaryFile(dir=args.journal.parent, delete=False) as stream:
                temporary = Path(stream.name)
                stream.write(json.dumps({'version': 1, 'state': state, 'target': str(target),
                    'sequence': release['sequence'], 'image_sha256': release['files']['system.raw']['sha256']}).encode())
                stream.flush(); os.fsync(stream.fileno())
            temporary.replace(args.journal)
            descriptor = os.open(args.journal.parent, os.O_DIRECTORY)
            try: os.fsync(descriptor)
            finally: os.close(descriptor)
        record('writing')
        flags = os.O_WRONLY | os.O_NOFOLLOW | (os.O_CREAT | os.O_EXCL if mode is None else 0)
        descriptor = os.open(target, flags, 0o600)
        try:
            with source.open('rb') as incoming, os.fdopen(descriptor, 'wb', closefd=False) as outgoing:
                shutil.copyfileobj(incoming, outgoing, length=1024**2)
                outgoing.flush(); os.fsync(descriptor)
        finally:
            os.close(descriptor)
        with target.open('rb') as stream:
            remaining = source.stat().st_size; actual = hashlib.sha256()
            while remaining:
                chunk = stream.read(min(1024**2, remaining))
                if not chunk: raise ValueError('short installed image')
                actual.update(chunk); remaining -= len(chunk)
        if actual.hexdigest() != release['files']['system.raw']['sha256']:
            raise ValueError('installed image verification failed; target is not bootable-approved')
        record('verified')
        return {'installed': str(target), 'sequence': release['sequence'], 'readback_verified': True,
                'source_channel': release['channel'], 'physical_boot_verified': False}
    finally:
        os.close(lock)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    operations = parser.add_subparsers(dest='operation', required=True)
    create = operations.add_parser('sign')
    create.add_argument('--image', type=Path)
    create.add_argument('--rootfs', type=Path)
    create.add_argument('--uki', type=Path)
    create.add_argument('--rootfs-sha256')
    create.add_argument('--rootfs-size', type=int)
    for name in ['private-key', 'output']:
        create.add_argument('--' + name, required=True, type=Path)
    create.add_argument('--version', required=True)
    create.add_argument('--sequence', required=True, type=int)
    create.add_argument('--inventory', type=Path)
    create.add_argument('--acceptance', type=Path)
    create.add_argument('--channel', choices=['development', 'release'], default='development')
    create.add_argument('--acceptance-signature', type=Path)
    create.add_argument('--acceptance-authority-public-key', type=Path)
    create.add_argument('--physical-recovery', type=Path)
    create.add_argument('--source-revision')
    create.add_argument('--contract-id')
    create.add_argument('--qualified-image-sha256')
    check = operations.add_parser('verify')
    deploy = operations.add_parser('install')
    for operation in [check, deploy]:
        operation.add_argument('--directory', required=True, type=Path)
        operation.add_argument('--public-key', required=True, type=Path)
        operation.add_argument('--acceptance-authority-public-key', type=Path)
        operation.add_argument('--require-release', action='store_true')
    deploy.add_argument('--target', required=True, type=Path)
    deploy.add_argument('--journal', required=True, type=Path)
    deploy.add_argument('--erase-all-data', action='store_true')
    deploy.add_argument('--expected-serial')
    args = parser.parse_args()
    result = sign(args) if args.operation == 'sign' else verify(args.directory, args.public_key, args.acceptance_authority_public_key, args.require_release) if args.operation == 'verify' else install(args)
    print(json.dumps(result))


if __name__ == '__main__':
    main()
