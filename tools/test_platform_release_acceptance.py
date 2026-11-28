"""Cryptographic acceptance tests use synthetic evidence and disposable test keys only."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from platform_release_acceptance import candidate, draft, verify_acceptance
from platform_physical_recovery import advance
from release_os_image import install, sign, verify


class PublicReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='cerynth-acceptance-fixture-')
        self.addCleanup(self.temporary.cleanup); self.root = Path(self.temporary.name)
        for name in ['publisher', 'authority', 'untrusted']:
            subprocess.run(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(self.root / (name + '.key'))], check=True, capture_output=True)
            subprocess.run(['openssl', 'pkey', '-in', str(self.root / (name + '.key')), '-pubout', '-out', str(self.root / (name + '.pub'))], check=True, capture_output=True)
        self.image = self.root / 'fixture.raw'; self.image.write_bytes(b'synthetic fixture only' * 4096)
        self.inventory = self.root / 'inventory.json'; self.inventory.write_text('{"test_fixture":true}')
        self.arguments = argparse.Namespace(image=self.image, rootfs=None, uki=None, rootfs_sha256=None, rootfs_size=None,
            private_key=self.root / 'publisher.key', output=self.root / 'development', inventory=self.inventory,
            acceptance=None, version='0.0.0-test-fixture-only', sequence=1)
        self.manifest = sign(self.arguments)
        self.subject = candidate(self.manifest, 'a' * 40, 'b' * 64)
        self.physical = {'schema_version': 1, 'kind': 'physical-image-recovery-checkpoints', 'test_fixture': True,
            'phase': 'complete', 'physical_boot_acceptance': True, 'image_sha256': self.subject['qualified_image_sha256'],
            'boot_ids_sha256': ['a' * 64, 'b' * 64, 'c' * 64], 'marker_sha256': 'd' * 64,
            'installer_journal_sha256': 'e' * 64, 'image_overlay_sha256': 'f' * 64,
            'operator_attested_power_cut': True, 'installation_readback_record_verified': True,
            'hardware_observation': {'virtualization': 'none', 'physical_host_observed': True,
                'expected_image_overlay_matches': True, 'image_overlay_sha256': 'f' * 64}}
        self.physical_path = self.root / 'physical.json'
        self.record = draft(self.subject)
        self.receipt = self.root / 'acceptance.json'; self.signature = self.root / 'acceptance.sig'

    def write_receipt(self, approve=True):
        self.physical_path.write_text(json.dumps(self.physical))
        self.record['physical_recovery_sha256'] = hashlib.sha256(self.physical_path.read_bytes()).hexdigest()
        if approve:
            self.record['approved'] = True
            for role, decision in self.record['reviews'].items():
                decision.update(approved=True, reviewer='synthetic-fixture-' + role,
                                evidence='https://example.invalid/synthetic-review/' + role)
        self.receipt.write_text(json.dumps(self.record))
        subprocess.run(['openssl', 'pkeyutl', '-sign', '-inkey', str(self.root / 'authority.key'), '-rawin',
                        '-in', str(self.receipt), '-out', str(self.signature)], check=True, capture_output=True)

    def check(self, key='authority.pub'):
        return verify_acceptance(self.receipt, self.signature, self.root / key, self.subject, self.physical_path)

    def publish_fixture(self):
        self.write_receipt(); self.check()
        args = copy.copy(self.arguments)
        args.channel = 'release'; args.output = self.root / 'public-fixture'
        args.acceptance = self.receipt; args.acceptance_signature = self.signature
        args.acceptance_authority_public_key = self.root / 'authority.pub'; args.physical_recovery = self.physical_path
        args.source_revision = 'a' * 40; args.contract_id = 'b' * 64
        return args, sign(args)

    def test_complete_signed_fixture_can_be_published_with_evidence_bound(self):
        args, result = self.publish_fixture()
        self.assertTrue(result['release_approved'])
        with self.assertRaisesRegex(ValueError, 'independently trusted'): verify(args.output, self.root / 'publisher.pub')
        self.assertEqual(verify(args.output, self.root / 'publisher.pub', self.root / 'authority.pub')['channel'], 'release')
        self.assertIn('acceptance.sig', result['files'])

    def test_publisher_key_alone_cannot_forge_acceptance(self):
        args, _ = self.publish_fixture()
        receipt = args.output / 'acceptance.json'
        forged = json.loads(receipt.read_text()); forged['reviews']['person2']['reviewer'] = 'substituted-reviewer'
        receipt.write_text(json.dumps(forged))
        manifest = json.loads((args.output / 'release.json').read_text())
        manifest['files']['acceptance.json'] = {'size': receipt.stat().st_size, 'sha256': hashlib.sha256(receipt.read_bytes()).hexdigest()}
        (args.output / 'release.json').write_text(json.dumps(manifest))
        subprocess.run(['openssl', 'pkeyutl', '-sign', '-inkey', str(self.root / 'publisher.key'), '-rawin',
            '-in', str(args.output / 'release.json'), '-out', str(args.output / 'release.sig')], check=True, capture_output=True)
        with self.assertRaises(subprocess.CalledProcessError):
            verify(args.output, self.root / 'publisher.pub', self.root / 'authority.pub')

    def test_public_installer_requires_both_trust_anchors_before_writing(self):
        args, _ = self.publish_fixture()
        deployment = argparse.Namespace(directory=args.output, public_key=self.root / 'publisher.pub',
            acceptance_authority_public_key=self.root / 'untrusted.pub', require_release=True,
            target=self.root / 'installed.raw', journal=self.root / 'install.json', erase_all_data=False, expected_serial=None)
        with self.assertRaises(subprocess.CalledProcessError): install(deployment)
        self.assertFalse(deployment.target.exists()); self.assertFalse(deployment.journal.exists())
        deployment.acceptance_authority_public_key = self.root / 'authority.pub'
        result = install(deployment)
        self.assertTrue(result['readback_verified']); self.assertEqual(result['source_channel'], 'release')
        self.assertEqual(deployment.target.read_bytes(), self.image.read_bytes())

    def test_missing_external_approval_refuses_public_signing_before_output(self):
        args = copy.copy(self.arguments); args.channel = 'release'; args.output = self.root / 'must-not-exist'
        with self.assertRaises(ValueError): sign(args)
        self.assertFalse(args.output.exists())

    def test_pending_template_is_not_an_approval_even_when_signed(self):
        self.write_receipt(approve=False)
        with self.assertRaisesRegex(ValueError, 'approval is missing'): self.check()

    def test_development_channel_cannot_bypass_required_public_acceptance(self):
        with self.assertRaisesRegex(ValueError, 'development artifact'):
            verify(self.arguments.output, self.root / 'publisher.pub', self.root / 'authority.pub', require_release=True)

    def test_untrusted_approval_signature_is_rejected(self):
        self.write_receipt()
        with self.assertRaises(subprocess.CalledProcessError): self.check('untrusted.pub')

    def test_another_commit_or_signer_cannot_reuse_approval(self):
        self.write_receipt()
        for key in ['source_revision', 'release_signer_sha256']:
            original = self.subject[key]; self.subject[key] = '0' * len(original)
            with self.assertRaisesRegex(ValueError, 'exact candidate'): self.check()
            self.subject[key] = original

    def test_physical_evidence_changed_after_approval_is_rejected(self):
        self.write_receipt(); self.physical_path.write_text('{}')
        with self.assertRaisesRegex(ValueError, 'physical evidence changed'): self.check()

    def test_vm_results_cannot_supply_physical_acceptance(self):
        self.physical['hardware_observation']['virtualization'] = 'kvm'; self.write_receipt()
        with self.assertRaisesRegex(ValueError, 'VM results'): self.check()

    def test_repeated_boot_or_wrong_image_is_rejected(self):
        original = copy.deepcopy(self.physical)
        for mutate in [lambda: self.physical.update(boot_ids_sha256=['a' * 64] * 3),
                       lambda: self.physical.update(image_sha256='0' * 64)]:
            self.physical = copy.deepcopy(original)
            mutate(); self.write_receipt()
            with self.assertRaises(ValueError): self.check()

    def test_omitted_team_or_license_review_is_rejected(self):
        self.write_receipt()
        for role in ['person2', 'licenses']:
            self.record['reviews'][role]['approved'] = False
            self.write_receipt(approve=False)
            with self.assertRaisesRegex(ValueError, 'missing identified approval'): self.check()
            self.record['reviews'][role]['approved'] = True


class PhysicalCheckpointTests(unittest.TestCase):
    def setUp(self):
        self.state = {'phase': 'prepared', 'marker_sha256': 'a' * 64, 'boot_ids_sha256': ['b' * 64]}

    def test_distinct_reboot_and_attested_power_cut_complete_checkpoints(self):
        reboot = advance(self.state, 'reboot-check', 'c' * 64, 'a' * 64, False)
        complete = advance(reboot, 'power-cut-check', 'd' * 64, 'a' * 64, True)
        self.assertTrue(complete['physical_boot_acceptance'])
        self.assertEqual(len(set(complete['boot_ids_sha256'])), 3)

    def test_same_boot_corruption_and_unattested_power_cut_are_rejected(self):
        with self.assertRaises(ValueError): advance(self.state, 'reboot-check', 'b' * 64, 'a' * 64, False)
        with self.assertRaises(ValueError): advance(self.state, 'reboot-check', 'c' * 64, '0' * 64, False)
        reboot = advance(self.state, 'reboot-check', 'c' * 64, 'a' * 64, False)
        with self.assertRaises(ValueError): advance(reboot, 'power-cut-check', 'd' * 64, 'a' * 64, False)

    def test_wrong_deployment_refuses_before_any_state_write(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / 'must-not-be-created'
            result = subprocess.run(['python3', str(Path(__file__).with_name('platform_physical_recovery.py')),
                'prepare', '--directory', str(directory), '--image-sha256', '0' * 64,
                '--expected-overlay-sha256', '0' * 64], capture_output=True, text=True, timeout=20)
            self.assertNotEqual(result.returncode, 0); self.assertFalse(directory.exists())


if __name__ == '__main__': unittest.main()
