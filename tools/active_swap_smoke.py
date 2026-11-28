#!/usr/bin/env python3
"""Exercise managed swap limits using an already-configured swap device.

This unprivileged fixture never enables swap or changes host VM settings.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import uuid
from guard_runtime_smoke import stop_process
from harness_runtime_smoke import rpc
from resource_hierarchy_smoke import admit_when_healthy, wait


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if len(Path('/proc/swaps').read_text().splitlines()) <= 1:
        raise SystemExit('An authorized test environment must already provide swap.')
    with tempfile.TemporaryDirectory(prefix='cerynth-active-swap-', dir='/var/tmp') as temporary:
        root = Path(temporary); state = root / 'runtime'
        process = subprocess.Popen([args.binary, '--directory', str(state), 'serve'],
            env=os.environ | {'CERYNTH_RESOURCE_HIERARCHY': '1', 'CERYNTH_AGENT_MEMORY_MAX_BYTES': str(512 * 1024**2)},
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            wait(lambda: (state / 'agentd.sock').exists())
            def create(name, swap):
                workspace = root / name; workspace.mkdir()
                return rpc(state, {'type': 'create', 'spec': {'name': name, 'harness': 'generic_pty',
                    'workspace': str(workspace), 'model': {'name': 'none'}, 'runtime': {'budget': {
                        'memory_max_bytes': 64 * 1024**2, 'memory_high_bytes': 64 * 1024**2,
                        'cpu_quota_micros_per_second': 1_000_000, 'timeout_seconds': 90, 'max_subagents': 0,
                        'platform': {'memory_swap_max_bytes': swap}}}}})
            def start(session):
                assert admit_when_healthy(state, session['id'])['decision'] == 'accept'
                code = 'import time\nprint("allocating",flush=True)\npayload=bytearray(100*1024**2)\nfor index in range(0,len(payload),4096): payload[index]=71\nprint("ready",flush=True)\ntime.sleep(65)'
                rpc(state, {'type': 'start', 'id': session['id'], 'operation_id': str(uuid.uuid4()), 'launch': {
                    'program': '/usr/bin/python3', 'args': ['-I', '-c', code], 'initial_input': None, 'pty': False}})
            allowed = create('swap-allowed', 96 * 1024**2); start(allowed)
            wait(lambda: 'ready' in rpc(state, {'type': 'output', 'id': allowed['id'], 'offset': 0, 'limit': 4096})['text'], timeout=60)
            observed = rpc(state, {'type': 'resource_accounting', 'id': allowed['id']})
            assert observed['advanced']['memory_swap_max_bytes'] == 96 * 1024**2, observed
            used = observed['advanced']['memory_swap_current_bytes']
            assert used is not None and 0 < used <= 96 * 1024**2, observed
            assert observed['memory_current_bytes'] <= 64 * 1024**2, observed
            rpc(state, {'type': 'stop', 'id': allowed['id']})
            wait(lambda: not rpc(state, {'type': 'status', 'id': allowed['id']})['execution']['resource_allocation']['active'])
            denied = create('swap-denied', 0); start(denied)
            before = rpc(state, {'type': 'status', 'id': denied['id']})
            wait(lambda: rpc(state, {'type': 'status', 'id': denied['id']})['state'] in ['completed', 'failed', 'cancelled'], timeout=60)
            finished = rpc(state, {'type': 'status', 'id': denied['id']})
            assert finished['state'] == 'failed', finished
            wait(lambda: not rpc(state, {'type': 'status', 'id': denied['id']})['execution']['resource_allocation']['active'])
            assert 'ready' not in rpc(state, {'type': 'output', 'id': denied['id'], 'offset': 0, 'limit': 4096})['text']
            report = {'schema_version': 1, 'kind': 'managed-active-swap-and-no-swap-control',
                'allowed_swap_limit_bytes': 96 * 1024**2, 'observed_swap_bytes': used,
                'resident_limit_bytes': 64 * 1024**2, 'observed_resident_bytes': observed['memory_current_bytes'],
                'payload_bytes': 100 * 1024**2, 'allowed_workload_reached_ready': True,
                'no_swap_control_failed_before_ready': True, 'no_swap_resource_slice': before['execution']['resource_slice'],
                'no_swap_failure_reason': finished['execution']['failure_reason'], 'cleanup_complete': True,
                'swap_configuration_modified_by_fixture': False}
            args.output.write_text(json.dumps(report, indent=2) + '\n'); print(json.dumps(report))
        finally: stop_process(process)


if __name__ == '__main__': main()
