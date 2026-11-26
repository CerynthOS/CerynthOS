#!/usr/bin/env python3
"""Validate retained platform evidence, including rejection of an unsafe experiment."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def load(name):
    return json.loads((ROOT / 'docs/benchmarks' / name).read_text())


def main():
    rebuild = load('person1-image-reproducibility.json')
    comparison = rebuild['comparison']
    assert rebuild['inputs_unchanged'] and rebuild['fresh_build_roots']
    assert comparison['byte_identical'] and comparison['mismatched_bytes'] == 0
    assert comparison['left_bytes'] == comparison['right_bytes'] > 1024**3
    assert comparison['left_sha256'] == comparison['right_sha256']
    assert rebuild['input_lock_sha256'] == hashlib.sha256((ROOT / 'docs/benchmarks/person1-image-inputs.json').read_bytes()).hexdigest()
    boot = load('person1-reproduced-image-boot.json')
    assert boot['image_sha256'] == comparison['left_sha256']
    assert boot['kernel_and_services'][1:] == ['active'] * 4
    assert boot['fsynced_marker_survived_power_cut'] and boot['advanced_resources']['affinity_and_io_qualified']
    tools = load('person1-tool-workloads.json')
    assert tools['fixture_sha256'] == hashlib.sha256((ROOT / 'tools/platform_workload_benchmark.py').read_bytes()).hexdigest()
    assert len(tools['observations']) == tools['repeats'] * 2 * 3
    assert all(o['verified'] and o['cgroup_cpu_usage_usec_delta'] > 0 for o in tools['observations'])
    assert tools['parent_cascade_cleanup'] and tools['hard_quotas_unchanged']
    control, experiment = [load(name) for name in ['person1-scx-bandwidth-linux.json', 'person1-scx-bandwidth.json']]
    fixture = hashlib.sha256((ROOT / 'tests/resource/scx_bandwidth_guest.c').read_bytes()).hexdigest()
    assert control['passed'] and not experiment['passed']
    assert control['fixture_sha256'] == experiment['fixture_sha256'] == fixture
    assert control['kernel_sha256'] == experiment['kernel_sha256']
    assert len(control['samples']) == len(experiment['samples']) == 9
    assert len([c for c in experiment['checks'] if not c['passed']]) == 6
    assert {c['name'] for c in experiment['checks'] if not c['passed']} == {
        'sustained_hierarchical_bandwidth_cap', 'individual_child_bandwidth_caps'}
    assert not experiment['host_scheduler_modified'] and not experiment['production_resource_gate_changed']
    assert experiment['build_provenance']['resolved_lock_sha256'] == hashlib.sha256((ROOT / 'docs/contracts/scx-bandwidth-candidate.lock').read_bytes()).hexdigest()
    print(json.dumps({'retained_evidence_consistent': True, 'base_disk_reproducible_from_frozen_payload': True,
                      'rebuilt_image_boot_qualified': True, 'real_tool_workflow_trials': len(tools['observations']),
                      'linux_nested_cpu_control_passed': True, 'experimental_scx_rejected': True,
                      'physical_acceptance_or_release_approval_claimed': False}))


if __name__ == '__main__':
    main()
