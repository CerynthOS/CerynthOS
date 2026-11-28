#!/usr/bin/env python3
"""Collect measured platform extension evidence without claiming release approval."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import subprocess
from qualify_platform import ROOT, source_manifest


def sha(path):
    with path.open('rb') as stream: return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--publish', action='store_true', help='also save the reviewable report under docs/benchmarks')
    args = parser.parse_args(); base = ROOT / 'artifacts/person1'
    inputs = {'inventory': 'platform-inventory-current.json', 'current_kernel_baseline': 'kernel-vm-current/result.json', 'host_advanced': 'advanced-resources.json',
        'workspace_capacity': 'workspace-quota.json', 'workspace_sessions': 'workspace-session.json',
        'phase_policy': 'phase-policy-benchmark.json', 'scx_experiment': 'scx-phase-comparison/report.json',
        'base_image': 'os-image-boot.json', 'desktop_image': 'os-image-desktop-boot.json',
        'ab_update': 'ab-update-qualification.json', 'signed_install': 'installed-image-boot.json',
        'signature_negative_tests': 'release-image-smoke.json', 'packaged_services': 'packaged-runtime-current.json',
        'shared_authority': 'shared-authority-current.json'}
    evidence = {key: json.loads((base / path).read_text()) for key, path in inputs.items()}
    image_paths = {'base_image': base / 'os-image/cerynthos.raw', 'desktop_image': base / 'os-image-desktop/cerynthos.raw',
                   'ab_update': base / 'os-image-ab/cerynthos.raw'}
    for key, path in image_paths.items():
        if evidence[key]['image_sha256'] != sha(path): raise ValueError('image was replaced after qualification: ' + key)
    if evidence['signed_install']['installed_sha256'] != evidence['base_image']['image_sha256']:
        raise ValueError('installed bytes differ from qualified source image')
    checks = {
        'frozen_baseline_unchanged': evidence['inventory']['baseline_integrity']['passed'],
        'current_frozen_binary_kernel_regressions': evidence['current_kernel_baseline']['passed'] and len(evidence['current_kernel_baseline']['checks']) >= 123,
        'workspace_enospc_and_crash_recovery': evidence['workspace_capacity']['enospc_observed'] and evidence['workspace_capacity']['interrupted_removal_recovered'],
        'sandbox_volume_attestation': evidence['workspace_sessions']['forged_workspace_metadata_rejected_before_process'],
        'advanced_kernel_controls': evidence['base_image']['advanced_resources']['affinity_and_io_qualified'],
        'desktop_under_managed_load': evidence['desktop_image']['desktop']['all_replies_within_two_seconds'],
        'boot_persistence': evidence['desktop_image']['fsynced_marker_survived_power_cut'],
        'ab_update_failure_and_recovery': all(evidence['ab_update'][name] for name in [
            'interrupted_inactive_write_kept_previous_root_bootable', 'unbootable_trial_fell_back_automatically',
            'healthy_trial_became_persistent_default', 'manual_rollback_persisted', 'explicit_configuration_and_home_var_preserved']),
        'signed_install_boot': evidence['signed_install']['installation_readback_verified'] and evidence['signed_install']['booted_installed_bytes'],
        'immutable_manifest_verification': evidence['signature_negative_tests']['manifest_substitution_after_signature_verification_prevented'],
        'packaged_service_recovery': evidence['packaged_services']['restart_on_failure_verified'],
        'shared_domain_guard_and_cleanup': evidence['shared_authority']['guard_and_shared_kernel_cleanup_verified'],
        'phase_preferences_stay_within_ceiling': evidence['phase_policy']['weights_within_owner_ceiling'],
        'scx_experiment_and_negative_checks': all(item['passed'] for item in evidence['scx_experiment']['checks']),
    }
    rust = Path('/tmp/cerynth-p1-workspace-tests.log'); python = Path('/tmp/cerynth-p1-python-tests.log')
    counts = re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored', rust.read_text())
    if not counts or 'test result: FAILED' in rust.read_text() or not python.read_text().rstrip().endswith('OK'):
        raise ValueError('test evidence is incomplete')
    checks['workspace_and_python_tests'] = True
    python_count = re.search(r'Ran (\d+) tests', python.read_text())
    if not python_count: raise ValueError('Python test count missing')
    if not all(checks.values()): raise ValueError('extension qualification failed: ' + repr(checks))
    report = {'schema_version': 1, 'observed_at': datetime.now(timezone.utc).isoformat(),
        'base_revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'qualified_source_sha256': source_manifest(), 'automated_extension_checks_passed': True, 'checks': checks,
        'rust_tests_passed': sum(int(item[0]) for item in counts), 'rust_tests_ignored': sum(int(item[2]) for item in counts),
        'python_tests_passed': int(python_count[1]), 'test_log_sha256': {'rust': sha(rust), 'python': sha(python)},
        'evidence_sha256': {key: sha(base / path) for key, path in inputs.items()}, 'evidence': evidence,
        'desktop_sbom_sha256': sha(base / 'desktop-sbom.cdx.json'),
        'physical_hardware_qualified': False, 'release_identity_approved': False, 'team_approval_claimed': False,
        'public_alpha_qualified': False,
        'external_acceptance_required': ['Authorized disposable physical installation/reboot target and hardware tests',
            'Authorized public release signing identity and human dependency/license review',
            'Coordinated acceptance of the candidate API/schema and release review'],
        'policy_limits': ['SCX cgroup quota/weight semantics remain unqualified; production resource control rejects SCX',
            'SCX snapshot result does not demonstrate a useful CPU-share improvement',
            'Full-image bit reproducibility, broad hardware support and representative agent speedups are not asserted']}
    destination = base / 'extension-qualification.json'; destination.write_text(json.dumps(report, indent=2) + '\n')
    if args.publish:
        (ROOT / 'docs/benchmarks/person1-platform-extensions.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: report[key] for key in ['automated_extension_checks_passed', 'rust_tests_passed', 'python_tests_passed', 'public_alpha_qualified']}))


if __name__ == '__main__': main()
