#!/usr/bin/env python3
"""Collect recovery evidence on an explicitly selected, already-installed physical image.

Does not install, reboot or cut power. Run the checkpoints around the operator's
authorized installation/reboot/power-cut procedure. A power cut is an explicit
operator attestation; software cannot distinguish it from a normal shutdown.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
import uuid
from platform_hardware_probe import probe
from release_os_image import bounded_regular_bytes, unique


def store(path, value):
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        stream.write(json.dumps(value, sort_keys=True, indent=2).encode() + b'\n')
        stream.flush(); os.fsync(stream.fileno())
    temporary.replace(path)
    descriptor = os.open(path.parent, os.O_DIRECTORY)
    try: os.fsync(descriptor)
    finally: os.close(descriptor)


def advance(state, phase, boot_hash, marker_hash, confirmed_power_cut):
    if marker_hash != state['marker_sha256']: raise ValueError('persistent marker changed')
    if phase == 'reboot-check':
        if state['phase'] != 'prepared' or boot_hash in state['boot_ids_sha256']:
            raise ValueError('a distinct reboot after prepare is required')
        return state | {'phase': 'reboot_verified', 'boot_ids_sha256': [*state['boot_ids_sha256'], boot_hash]}
    if phase != 'power-cut-check' or state['phase'] != 'reboot_verified' or not confirmed_power_cut:
        raise ValueError('reboot checkpoint and explicit operator power-cut attestation required')
    if boot_hash in state['boot_ids_sha256']: raise ValueError('a distinct boot after the power cut is required')
    return state | {'phase': 'complete', 'boot_ids_sha256': [*state['boot_ids_sha256'], boot_hash],
                    'operator_attested_power_cut': True, 'physical_boot_acceptance': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase', choices=['prepare', 'reboot-check', 'power-cut-check'])
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--image-sha256', required=True)
    parser.add_argument('--expected-overlay-sha256', required=True)
    parser.add_argument('--install-journal', type=Path)
    parser.add_argument('--operator-confirmed-power-cut', action='store_true')
    args = parser.parse_args()
    for value in [args.image_sha256, args.expected_overlay_sha256]:
        if len(value) != 64 or any(character not in '0123456789abcdef' for character in value):
            parser.error('image and overlay identities must be lowercase SHA256 values')
    observation = probe(args.expected_overlay_sha256)
    if not observation['physical_host_observed'] or not observation['expected_image_overlay_matches']:
        raise SystemExit('Refusing recovery writes: this is not the selected Cerynth physical image.')
    if os.geteuid() != 0: raise SystemExit('Run as the installed test image administrator.')
    directory = args.directory.absolute()
    if directory == Path('/'): raise ValueError('select a private qualification directory')
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = directory.lstat()
    if not directory.is_dir() or directory.is_symlink() or info.st_uid != 0 or info.st_mode & 0o777 != 0o700:
        raise ValueError('qualification directory must be root-owned, private and non-symlink')
    import fcntl
    descriptor = os.open(directory / 'writer.lock', os.O_CREAT | os.O_NOFOLLOW | os.O_RDWR, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        boot_hash = hashlib.sha256(Path('/proc/sys/kernel/random/boot_id').read_bytes()).hexdigest()
        state_path = directory / 'recovery.json'
        if args.phase == 'prepare':
            if state_path.exists(): raise ValueError('select a new run directory')
            if args.install_journal is None: raise ValueError('supply the verified physical installer journal')
            journal_info = args.install_journal.lstat()
            if journal_info.st_uid != 0 or journal_info.st_mode & 0o777 != 0o600:
                raise ValueError('installer journal must be root-owned and private')
            journal_bytes = bounded_regular_bytes(args.install_journal, 65536)
            journal = json.loads(journal_bytes, object_pairs_hook=unique)
            if journal.get('state') != 'verified' or journal.get('image_sha256') != args.image_sha256:
                raise ValueError('installer journal does not attest the selected image readback')
            marker = os.urandom(4096)
            marker_descriptor = os.open(directory / 'marker', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            with os.fdopen(marker_descriptor, 'wb') as stream:
                stream.write(marker); stream.flush(); os.fsync(stream.fileno())
            state = {'schema_version': 1, 'kind': 'physical-image-recovery-checkpoints',
                'run_id': str(uuid.uuid4()), 'phase': 'prepared', 'image_sha256': args.image_sha256,
                'image_overlay_sha256': args.expected_overlay_sha256,
                'installer_journal_sha256': hashlib.sha256(journal_bytes).hexdigest(),
                'installation_readback_record_verified': True, 'marker_sha256': hashlib.sha256(marker).hexdigest(),
                'boot_ids_sha256': [boot_hash], 'operator_attested_power_cut': False, 'physical_boot_acceptance': False}
        else:
            state = json.loads(bounded_regular_bytes(state_path, 1024**2), object_pairs_hook=unique)
            if state['image_sha256'] != args.image_sha256 or state['image_overlay_sha256'] != args.expected_overlay_sha256:
                raise ValueError('checkpoint belongs to another image')
            state = advance(state, args.phase, boot_hash, hashlib.sha256(bounded_regular_bytes(directory / 'marker', 4096)).hexdigest(), args.operator_confirmed_power_cut)
        state['hardware_observation'] = observation
        store(state_path, state)
        print(json.dumps({'report': str(state_path), 'phase': state['phase'], 'physical_boot_acceptance': state['physical_boot_acceptance'],
                          'note': 'No installation, reboot or power cut was performed by this command.'}))
    finally: os.close(descriptor)


if __name__ == '__main__': main()
