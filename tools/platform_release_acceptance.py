#!/usr/bin/env python3
"""Validate externally signed acceptance for one exact public-release candidate.

Templates contain no approvals. The approval-authority private key is never
accepted by this command. Public release signing uses a separate signing step.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
from release_os_image import bounded_regular_bytes, unique, verify

EVIDENCE_FILES = {'acceptance.json', 'acceptance.sig', 'physical-recovery.json'}
ROLES = ('person1', 'person2', 'person4', 'licenses', 'release')


def hash_value(value):
    return isinstance(value, str) and re.fullmatch('[a-f0-9]{64}', value) is not None


def candidate(manifest, source_revision, contract_id, qualified_image=None):
    if not isinstance(source_revision, str) or not re.fullmatch('[a-f0-9]{40}', source_revision):
        raise ValueError('an exact source commit is required')
    if not hash_value(contract_id): raise ValueError('an exact API contract fingerprint is required')
    payload = {name: entry['sha256'] for name, entry in manifest['files'].items() if name not in EVIDENCE_FILES}
    image = payload.get('system.raw') or qualified_image
    if not hash_value(image) or (qualified_image is not None and image != qualified_image):
        raise ValueError('identify the qualified complete OS image')
    if 'packages.json' not in payload: raise ValueError('include the dependency/license inventory')
    return {'source_revision': source_revision, 'contract_id': contract_id, 'version': manifest['version'],
            'sequence': manifest['sequence'], 'qualified_image_sha256': image, 'payload_sha256': payload,
            'release_signer_sha256': manifest['public_key_sha256']}


def draft(subject):
    return {'schema_version': 1, 'audience': 'cerynthos-public-release', 'candidate': subject,
            'approved': False, 'physical_recovery_sha256': None,
            'reviews': {role: {'approved': False, 'reviewer': '', 'evidence': ''} for role in ROLES},
            'note': 'Complete only after the actual reviews and physical procedure. Sign offline with the configured approval authority.'}


def validate(record, subject, physical, physical_sha):
    if record.get('schema_version') != 1 or record.get('audience') != 'cerynthos-public-release':
        raise ValueError('acceptance belongs to another protocol or purpose')
    if record.get('candidate') != subject: raise ValueError('acceptance does not match this exact candidate')
    if record.get('approved') is not True: raise ValueError('release approval is missing')
    reviews = record.get('reviews', {})
    for role in ROLES:
        review = reviews.get(role, {})
        if review.get('approved') is not True or not isinstance(review.get('reviewer'), str) or not review['reviewer'].strip():
            raise ValueError('missing identified approval: ' + role)
        reference = review.get('evidence')
        if not isinstance(reference, str) or not 1 <= len(reference) <= 4096 or not reference.startswith(('https://', 'sha256:')):
            raise ValueError('missing review evidence: ' + role)
    if record.get('physical_recovery_sha256') != physical_sha: raise ValueError('physical evidence changed')
    if physical.get('kind') != 'physical-image-recovery-checkpoints' or physical.get('schema_version') != 1:
        raise ValueError('physical recovery checkpoints required')
    if physical.get('phase') != 'complete' or physical.get('physical_boot_acceptance') is not True:
        raise ValueError('physical recovery is incomplete')
    if physical.get('image_sha256') != subject['qualified_image_sha256']:
        raise ValueError('physical tests used another image')
    boots = physical.get('boot_ids_sha256', [])
    if len(boots) != 3 or not all(hash_value(value) for value in boots) or len(set(boots)) != 3:
        raise ValueError('three distinct boot checkpoints required')
    if physical.get('operator_attested_power_cut') is not True or physical.get('installation_readback_record_verified') is not True:
        raise ValueError('physical installation/power-cut evidence missing')
    if not hash_value(physical.get('marker_sha256')) or not hash_value(physical.get('installer_journal_sha256')):
        raise ValueError('persistent marker or installer identity missing')
    hardware = physical.get('hardware_observation', {})
    if hardware.get('virtualization') != 'none' or hardware.get('physical_host_observed') is not True:
        raise ValueError('VM results cannot supply physical acceptance')
    if hardware.get('expected_image_overlay_matches') is not True or not hash_value(physical.get('image_overlay_sha256')) or hardware.get('image_overlay_sha256') != physical['image_overlay_sha256']:
        raise ValueError('the deployed image payload is not identified')


def verify_acceptance(receipt, signature, authority_key, subject, physical_report):
    metadata = bounded_regular_bytes(receipt, 1024**2)
    signature_bytes = bounded_regular_bytes(signature, 64)
    if len(signature_bytes) != 64: raise ValueError('an Ed25519 acceptance signature is required')
    with tempfile.TemporaryDirectory(prefix='cerynth-acceptance-') as temporary:
        root = Path(temporary); (root / 'receipt').write_bytes(metadata); (root / 'signature').write_bytes(signature_bytes)
        subprocess.run(['openssl', 'pkeyutl', '-verify', '-pubin', '-inkey', str(authority_key), '-rawin',
                        '-in', str(root / 'receipt'), '-sigfile', str(root / 'signature')],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    physical_bytes = bounded_regular_bytes(physical_report, 1024**2)
    validate(json.loads(metadata, object_pairs_hook=unique), subject,
             json.loads(physical_bytes, object_pairs_hook=unique), hashlib.sha256(physical_bytes).hexdigest())
    public_der = subprocess.check_output(['openssl', 'pkey', '-pubin', '-in', str(authority_key), '-outform', 'DER'], stderr=subprocess.PIPE)
    return {'acceptance_sha256': hashlib.sha256(metadata).hexdigest(),
            'acceptance_authority_sha256': hashlib.sha256(public_der).hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['template', 'check'])
    parser.add_argument('--bundle', type=Path, required=True)
    parser.add_argument('--bundle-public-key', type=Path, required=True)
    parser.add_argument('--source-revision', required=True)
    parser.add_argument('--contract-id', required=True)
    parser.add_argument('--qualified-image-sha256')
    parser.add_argument('--release-signer-public-key', type=Path, help='select the real publisher identity before human review; no private key is read')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--receipt', type=Path)
    parser.add_argument('--signature', type=Path)
    parser.add_argument('--authority-public-key', type=Path)
    parser.add_argument('--physical-report', type=Path)
    args = parser.parse_args()
    subject = candidate(verify(args.bundle, args.bundle_public_key, args.authority_public_key), args.source_revision,
                        args.contract_id, args.qualified_image_sha256)
    if args.release_signer_public_key:
        public_der = subprocess.check_output(['openssl', 'pkey', '-pubin', '-in', str(args.release_signer_public_key), '-outform', 'DER'], stderr=subprocess.PIPE)
        subject['release_signer_sha256'] = hashlib.sha256(public_der).hexdigest()
    if args.operation == 'template':
        if args.output is None: parser.error('template requires --output')
        args.output.parent.mkdir(parents=True, exist_ok=True)
        # An existing reviewed receipt must never be overwritten by a template.
        with args.output.open('x') as stream: stream.write(json.dumps(draft(subject), indent=2) + '\n')
        print(json.dumps({'output': str(args.output), 'approved': False})); return
    if not all([args.receipt, args.signature, args.authority_public_key, args.physical_report]):
        parser.error('check requires receipt, signature, authority public key and physical report')
    result = verify_acceptance(args.receipt, args.signature, args.authority_public_key, subject, args.physical_report)
    print(json.dumps(result | {'acceptance_verified': True}))


if __name__ == '__main__': main()
