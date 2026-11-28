#!/usr/bin/env python3
"""Measure actual KWin D-Bus replies under bounded managed CPU load."""
import argparse
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid
from harness_runtime_smoke import rpc
from guard_runtime_smoke import stop_process
from resource_hierarchy_smoke import wait, admit_when_healthy


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='/usr/lib/cerynth/cerynth-agentd')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='cerynth-desktop-load-', dir='/var/tmp') as temporary:
        root = Path(temporary); state = root / 'runtime'
        process = subprocess.Popen([args.binary, '--directory', str(state), 'serve'],
            env=os.environ | {'CERYNTH_RESOURCE_HIERARCHY': '1', 'CERYNTH_AGENT_MEMORY_MAX_BYTES': str(512 * 1024**2)},
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            wait(lambda: (state / 'agentd.sock').exists())
            sessions = []
            for index in range(2):
                workspace = root / str(index); workspace.mkdir()
                session = rpc(state, {'type': 'create', 'spec': {'name': f'desktop-load-{index}', 'harness': 'generic_pty',
                    'workspace': str(workspace), 'model': {'name': 'none'}, 'runtime': {'budget': {
                    'memory_max_bytes': 128 * 1024**2, 'memory_high_bytes': 96 * 1024**2,
                    'cpu_quota_micros_per_second': 500_000, 'timeout_seconds': 60, 'max_subagents': 0}}}})
                assert admit_when_healthy(state, session['id'])['decision'] == 'accept'
                rpc(state, {'type': 'start', 'id': session['id'], 'operation_id': str(uuid.uuid4()), 'launch': {
                    'program': '/usr/bin/python3', 'args': ['-I', '-c', 'buffer=bytearray(32*1024**2)\nprint("ready",flush=True)\nwhile True: sum(range(10000))'], 'initial_input': None, 'pty': False}})
                wait(lambda: 'ready' in rpc(state, {'type': 'output', 'id': session['id'], 'offset': 0, 'limit': 4096})['text'])
                sessions.append(session)
            latency = []
            for _ in range(30):
                before = time.monotonic()
                subprocess.run(['busctl', '--user', '--timeout=2', 'call', 'org.kde.KWin', '/KWin',
                                'org.freedesktop.DBus.Peer', 'Ping'], check=True, capture_output=True, timeout=3)
                latency.append((time.monotonic() - before) * 1000)
                time.sleep(.05)
            accounting = [rpc(state, {'type': 'resource_accounting', 'id': session['id']}) for session in sessions]
            assert all(item['cpu_stat']['usage_usec'] > 0 for item in accounting), accounting
            for session in sessions: rpc(state, {'type': 'stop', 'id': session['id']})
            wait(lambda: all(not rpc(state, {'type': 'status', 'id': session['id']})['execution']['resource_allocation']['active'] for session in sessions))
            report = {'schema_version': 1, 'kind': 'kwin-dbus-under-managed-cpu-load', 'samples_ms': latency,
                      'p50_ms': sorted(latency)[14], 'p95_ms': sorted(latency)[math.ceil(.95 * len(latency)) - 1],
                      'managed_cpu_microseconds': [item['cpu_stat']['usage_usec'] for item in accounting],
                      'all_replies_within_two_seconds': True, 'sessions_cleaned_up': True,
                      'scope': 'VM D-Bus service responsiveness; not physical input-to-photon latency'}
            args.output.write_text(json.dumps(report, indent=2) + '\n'); print(json.dumps(report))
        finally: stop_process(process)


if __name__ == '__main__': main()
