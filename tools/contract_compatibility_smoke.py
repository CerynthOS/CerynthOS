#!/usr/bin/env python3
"""Live contract pinning: reject mismatched clients before a registry mutation."""
import argparse
import json
import os
import socket
import subprocess
import tempfile
from pathlib import Path

from guard_runtime_smoke import stop_process
from harness_runtime_smoke import rpc
from resource_hierarchy_smoke import wait


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', default='artifacts/person3/contract-compatibility-smoke.json')
    args = parser.parse_args()
    binary = str(Path('target/debug/cerynth-agentd').resolve())
    with tempfile.TemporaryDirectory(prefix='cerynth-contract-') as temporary:
        root = Path(temporary)
        directory = root / 'state'
        workspace = root / 'workspace'
        workspace.mkdir()
        process = subprocess.Popen([binary, '--directory', str(directory), 'serve'],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            def ready():
                assert process.poll() is None, process.stderr.read().decode()
                try:
                    return rpc(directory, {'type': 'protocol'})
                except (OSError, ValueError):
                    return None
            protocol = wait(ready)
            assert protocol['team_approval_claimed'] is False
            assert len(protocol['contract_id']) == 64
            spec = root / 'session.json'
            spec.write_text(json.dumps({'name': 'public-contract-fixture', 'harness': 'generic_pty',
                                        'workspace': str(workspace), 'model': {'name': 'none'}}))
            initial = rpc(directory, {'type': 'list'})
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.settimeout(10)
                stream.connect(str(directory / 'agentd.sock'))
                stream.sendall(json.dumps({'version': 1, 'expected_contract': 'f' * 64,
                    'operation': {'type': 'create', 'spec': json.loads(spec.read_text())}}).encode() + b'\n')
                rejected_raw = json.loads(stream.makefile('rb').readline())
            assert 'operation not dispatched' in rejected_raw['result']['Err']
            assert rpc(directory, {'type': 'list'}) == initial
            command = [binary, '--directory', str(directory), 'create-spec', str(spec)]
            rejected = subprocess.run(command, env={**os.environ, 'CERYNTH_AGENT_CONTRACT': '0' * 64},
                                      capture_output=True, text=True, timeout=10)
            assert rejected.returncode != 0 and 'operation not dispatched' in rejected.stderr
            assert rpc(directory, {'type': 'list'}) == initial, 'mismatched client mutated registry'
            accepted = subprocess.run(command, env={**os.environ, 'CERYNTH_AGENT_CONTRACT': protocol['contract_id']},
                                     capture_output=True, text=True, timeout=10, check=True)
            session = json.loads(accepted.stdout)
            proposal = rpc(directory, {'type': 'plan_global_resources', 'id': session['id'],
                                       'minimum': None, 'gpu_device_id': None})
            assert proposal['proposal']['decision'] == 'queue' and proposal['applied'] is False
            assert proposal['proposal']['reason'] == 'global_accounting_incomplete'
            report = {'kind': 'live-owner-client-contract-pinning', 'contract_id': protocol['contract_id'],
                      'wrong_contract_rejected_before_mutation': True, 'matching_contract_created_session': True,
                      'contract_pin_in_mutating_request_checked_by_server': True,
                      'raw_wrong_pin_rejected_without_registry_mutation': True,
                      'instance_only_global_proposal_queues_without_actuation': True,
                      'team_approval_claimed': False, 'cloud_requests': 0}
            destination = Path(args.output)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(report, indent=2))
        finally:
            stop_process(process)


if __name__ == '__main__':
    main()
