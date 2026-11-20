#!/usr/bin/env python3
"""Two real agent daemons share durable root budgets and recover only their scopes.

This is a managed-domain test. No claim covers unregistered model servers,
ordinary desktop apps, bypassing GPU clients, or hardware VRAM partitioning.
"""
import argparse
import json
import os
import shutil
from pathlib import Path
import subprocess
import tempfile
import time
import uuid

from guard_runtime_smoke import stop_process
from harness_runtime_smoke import rpc
from resource_hierarchy_smoke import wait


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', default='artifacts/person3/shared-authority-smoke.json')
    args = parser.parse_args()
    binaries = {name: str(Path('target/debug/cerynth-' + name).resolve())
                for name in ['agentd', 'resource-authority', 'guardd', 'guard-exec']}
    gpu_device = subprocess.check_output(['nvidia-smi', '--query-gpu=uuid', '--format=csv,noheader'], text=True).splitlines()[0].strip()
    with tempfile.TemporaryDirectory(prefix='cerynth-shared-') as temporary:
        root = Path(temporary)
        authority = root / 'authority'
        policy = root / 'policy.json'
        policy.write_text(json.dumps({'version': 1, 'capacity': {'memory_bytes': 768 * 1024**2,
            'cpu_quota_micros_per_second': 2_000_000}, 'inference_limit': 2,
            'accelerators': {gpu_device: {'total_bytes': 1024**3, 'desktop_reserve_bytes': 256 * 1024**2}}}))
        subprocess.run([binaries['resource-authority'], 'init', '--directory', str(authority), '--policy', str(policy)], check=True)
        guard_run = root / 'guard'
        guard_config = root / 'guard.toml'
        guard_config.write_text(f'runtime_dir = {json.dumps(str(guard_run))}\nstate_dir = {json.dumps(str(root / "security"))}\n'
            f'guard_exec = {json.dumps(binaries["guard-exec"])}\n')
        guard = subprocess.Popen([binaries['guardd'], 'serve', str(guard_config)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        environment = {**os.environ, 'CERYNTH_RESOURCE_HIERARCHY': '1',
            'CERYNTH_RESOURCE_AUTHORITY_DIR': str(authority), 'CERYNTH_AGENT_MEMORY_MAX_BYTES': str(768 * 1024**2),
            'CERYNTH_GUARD_SOCKET': str(guard_run / 'guard.sock')}
        states = [root / 'a', root / 'b']
        processes = [None, None]

        def launch(index, omit_authority=False):
            selected = {k: v for k, v in environment.items() if not (omit_authority and k == 'CERYNTH_RESOURCE_AUTHORITY_DIR')}
            process = subprocess.Popen([binaries['agentd'], '--directory', str(states[index]), 'serve'],
                env=selected, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            def ready():
                assert process.poll() is None, process.stderr.read().decode()
                try:
                    return rpc(states[index], {'type': 'diagnostics'})
                except (OSError, ValueError):
                    return None
            try:
                wait(ready)
            except BaseException:
                stop_process(process)
                raise
            return process

        def create(index, name, memory=512, inference=1, gpu=0, parent=None, workspace=None, guard_policy=None):
            workspace = workspace or root / name
            workspace.mkdir(exist_ok=True)
            return rpc(states[index], {'type': 'create', 'spec': {'name': name, 'harness': 'generic_pty',
                'workspace': str(workspace), 'model': {'name': 'none'}, 'runtime': {'parent_id': parent,
                'capabilities': ['gpu.execute'], 'guard': guard_policy, 'budget': {'cpu_quota_micros_per_second': 1_000_000,
                    'memory_max_bytes': memory * 1024**2, 'memory_high_bytes': min(memory, 128) * 1024**2,
                    'inference_concurrency': inference, 'gpu_memory_bytes': gpu * 1024**2,
                    'timeout_seconds': 180, 'max_subagents': 2}}}})

        def admit(index, session, minimum=None):
            def attempt():
                try:
                    result = rpc(states[index], {'type': 'resource_admit', 'id': session['id'], 'minimum': minimum})
                except RuntimeError as error:
                    if 'authority transaction busy' in str(error):
                        return None
                    raise
                return None if result.get('reason') == 'resource_pressure' else result
            return wait(attempt, timeout=90)

        def claims():
            return rpc(states[1], {'type': 'resource_view'})['shared_authority']['claims']

        def stop(index, session):
            rpc(states[index], {'type': 'stop', 'id': session['id']})
            def drained():
                record = rpc(states[index], {'type': 'status', 'id': session['id']})
                return record if record['state'] in ['cancelled', 'completed', 'failed'] and not record['execution']['resource_allocation']['active'] else None
            return wait(drained)

        try:
            wait(lambda: (guard_run / 'guard.sock').exists())
            for i in range(2):
                processes[i] = launch(i)
            first = create(0, 'first')
            other = create(1, 'other')
            assert admit(0, first)['decision'] == 'accept'
            try:
                rpc(states[0], {'type': 'observe', 'id': first['id'], 'pid': os.getpid()})
                raise AssertionError('observed host process inherited managed reservation')
            except RuntimeError as error:
                assert 'managed resource allocation' in str(error)
            child = create(0, 'child', memory=128, parent=first['id'])
            assert admit(0, child)['decision'] == 'accept'
            for session in [first, child]:
                rpc(states[0], {'type': 'start', 'id': session['id'], 'operation_id': str(uuid.uuid4()),
                    'launch': {'program': '/usr/bin/python3', 'args': ['-I', '-c', 'import time;print("ready",flush=True);time.sleep(170)'],
                               'initial_input': None, 'pty': False}})
                wait(lambda: 'ready' in rpc(states[0], {'type': 'output', 'id': session['id'], 'offset': 0, 'limit': 4096})['text'])
            assert len(claims()) == 1 and first['id'] in claims(), 'child was double charged'
            original = rpc(states[0], {'type': 'status', 'id': first['id']})
            moved = root / 'copied-member'
            moved.mkdir(mode=0o700)
            for name in ['sessions.json', 'resource-authority.json']:
                shutil.copyfile(states[0] / name, moved / name)
                (moved / name).chmod(0o600)
            rejected = subprocess.run([binaries['agentd'], '--directory', str(moved), 'serve'],
                env=environment, capture_output=True, text=True, timeout=15)
            assert rejected.returncode != 0 and 'member identity changed' in rejected.stderr
            os.kill(original['execution']['process']['pid'], 0)
            assert rpc(states[0], {'type': 'status', 'id': first['id']})['state'] == 'running'
            assert admit(1, other)['decision'] == 'queue', 'independent daemon overcommitted shared memory'
            processes[0].kill()
            processes[0].wait(timeout=10)
            assert first['id'] in claims() and admit(1, other)['decision'] == 'queue', 'dead owner was unsafely reaped'
            processes[0] = launch(0, omit_authority=True)
            assert not claims(), 'verified recovery did not release the retained root'
            assert rpc(states[0], {'type': 'status', 'id': first['id']})['state'] == 'failed'
            assert admit(1, other)['decision'] == 'accept'
            stop(1, other)

            cpu_a = create(0, 'cpu-one', memory=128, inference=0)
            cpu_b = create(1, 'cpu-two', memory=128, inference=0)
            cpu_waiting = create(0, 'cpu-three', memory=128, inference=0)
            assert admit(0, cpu_a)['decision'] == admit(1, cpu_b)['decision'] == 'accept'
            assert admit(0, cpu_waiting)['decision'] == 'queue', 'CPU admission ignored another member while RAM was available'
            stop(0, cpu_a)
            assert admit(0, cpu_waiting)['decision'] == 'accept'
            stop(0, cpu_waiting)
            stop(1, cpu_b)

            one = create(0, 'inference-one', memory=128, inference=2)
            two = create(1, 'inference-two', memory=128)
            assert admit(0, one)['decision'] == 'accept'
            queued = admit(1, two)
            assert queued['decision'] == 'queue' and queued['reason'] == 'inference_concurrency_reserved', queued
            stop(0, one)
            assert admit(1, two)['decision'] == 'accept'
            stop(1, two)

            ga = create(0, 'gpu-first', memory=128, inference=0, gpu=512)
            gb = create(1, 'gpu-second', memory=128, inference=0, gpu=512)
            assert admit(0, ga)['decision'] == 'accept'
            assert admit(1, gb)['decision'] == 'queue'
            minimum = {**gb['spec']['runtime']['budget'], 'gpu_memory_bytes': 256 * 1024**2}
            reduced = admit(1, gb, minimum)
            assert reduced['decision'] == 'reduce' and reduced['allocation']['granted']['gpu_memory_bytes'] == 256 * 1024**2
            gc = create(0, 'gpu-child', memory=64, inference=0, gpu=256, parent=ga['id'])
            assert admit(0, gc)['decision'] == 'accept'
            job = rpc(states[0], {'type': 'gpu_submit', 'id': ga['id'], 'device_id': gpu_device, 'memory_bytes': 384 * 1024**2, 'priority': 0})
            try:
                rpc(states[0], {'type': 'gpu_submit', 'id': gc['id'], 'device_id': gpu_device, 'memory_bytes': 256 * 1024**2, 'priority': 0})
                raise AssertionError('parent plus child exceeded shared GPU root reservation')
            except RuntimeError as error:
                assert 'ancestor reservation' in str(error)
            rpc(states[0], {'type': 'gpu_finish', 'id': ga['id'], 'job_id': job['id'], 'succeeded': False})
            stop(0, ga)
            stop(1, gb)
            assert not claims()
            exposed = create(0, 'exposed-control', memory=64, inference=0, workspace=authority)
            assert admit(0, exposed)['decision'] == 'accept'
            try:
                rpc(states[0], {'type': 'start', 'id': exposed['id'], 'operation_id': str(uuid.uuid4()),
                    'launch': {'program': '/usr/bin/true', 'args': [], 'initial_input': None, 'pty': False}})
                raise AssertionError('authority control directory mounted into agent workspace')
            except RuntimeError as error:
                assert 'control directory' in str(error)
            # Early launch rejection leaves the explicitly admitted Created
            # session reserved until the owner cancels it.
            assert exposed['id'] in claims()
            stop(0, exposed)
            forbidden = create(0, 'forbidden-guard-grant', memory=64, inference=0,
                guard_policy={'capabilities': [{'kind': 'fs.read', 'target': str(authority)}]})
            assert admit(0, forbidden)['decision'] == 'accept'
            try:
                rpc(states[0], {'type': 'start', 'id': forbidden['id'], 'operation_id': str(uuid.uuid4()),
                    'launch': {'program': '/usr/bin/true', 'args': [], 'initial_input': None, 'pty': False}})
                raise AssertionError('Guard filesystem grant exposed shared authority')
            except RuntimeError as error:
                assert 'control directories' in str(error)
            allowed = create(0, 'allowed-guard-task', memory=64, inference=0,
                guard_policy={'capabilities': [{'kind': 'fs.read', 'target': 'workspace'}]})
            assert admit(0, allowed)['decision'] == 'accept'
            rpc(states[0], {'type': 'start', 'id': allowed['id'], 'operation_id': str(uuid.uuid4()),
                'launch': {'program': '/usr/bin/true', 'args': [], 'initial_input': None, 'pty': False}})
            def guard_drained():
                record = rpc(states[0], {'type': 'status', 'id': allowed['id']})
                return record if record['state'] == 'completed' and not record['execution']['guard_cleanup_pending'] and not record['execution']['resource_allocation']['active'] else None
            assert wait(guard_drained)['boundary'] == 'guard_systemd'
            assert not claims()
            assert rpc(states[0], {'type': 'archive', 'id': exposed['id']})['metadata_preserved']
            report = {'kind': 'real-two-agentd-shared-reservation-domain', 'daemons': 2,
                'cross_instance_memory_and_cpu_reservations': True, 'child_not_double_charged': True,
                'shared_cpu_quota_gate_with_available_ram': True,
                'observed_process_cannot_inherit_managed_allocation': True,
                'crashed_member_claim_retained_until_verified_recovery': True, 'saved_authority_binding_used_on_restart': True,
                'copied_member_rejected_before_original_scope_cleanup': True,
                'global_inference_concurrency_reservations': True, 'live_gpu_device_cooperative_reservations': True,
                'reduced_gpu_grant_preserves_owner_minimum': True, 'parent_plus_child_gpu_jobs_bound_by_root': True,
                'authority_workspace_exposure_rejected': True, 'cloud_requests': 0, 'team_approval_claimed': False,
                'guard_filesystem_grant_cannot_expose_authority': True, 'guard_and_shared_kernel_cleanup_verified': True,
                'terminal_session_archival_checked': True,
                'scope': 'Participating agentd root subtrees only; unmanaged model servers and bypassing clients excluded',
                'gpu_enforcement': 'cooperative_reservation; no hardware partition claim'}
            destination = Path(args.output)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(report, indent=2))
        finally:
            for process in processes:
                if process is not None:
                    stop_process(process)
            stop_process(guard)


if __name__ == '__main__':
    main()
