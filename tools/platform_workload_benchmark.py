#!/usr/bin/env python3
"""Compare phase preferences using real build/test and artifact tools in managed sessions."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import tempfile
import time
import uuid


def checked(command, cwd=None):
    result = subprocess.run(command, cwd=cwd, check=True, capture_output=True, text=True, timeout=90)
    return result.stdout


def prepare(workspace):
    """Public deterministic fixture; no private repository content or network access."""
    sources = workspace / 'src'; sources.mkdir()
    declarations = []
    # Many independent functions exercise an actual optimizing C compiler/linker.
    for unit in range(12):
        functions = []
        for index in range(90):
            name = f'f_{unit}_{index}'
            functions.append(f'unsigned {name}(unsigned x){{ for(unsigned i=0;i<32;i++) x=(x*1664525u+1013904223u)^(x>>7); return x; }}')
        (sources / f'unit{unit}.c').write_text('\n'.join(functions) + '\n')
        declarations.append(f'extern unsigned f_{unit}_0(unsigned);')
    expected = 7
    for _ in range(12 * 32):
        expected = ((expected * 1664525 + 1013904223) & 0xffffffff) ^ (expected >> 7)
    (sources / 'main.c').write_text('\n'.join(declarations) + '\nint main(void){ unsigned x=7;\n' +
        ''.join(f'x=f_{unit}_0(x);\n' for unit in range(12)) + f'return x!={expected}u; }}\n')
    (workspace / 'arithmetic.py').write_text('def add(a,b):\n    return a-b\n')
    (workspace / 'test_arithmetic.py').write_text('import unittest\nfrom arithmetic import add\nclass Tests(unittest.TestCase):\n' +
        ''.join(f' def test_{i}(self): self.assertEqual(add({i},{i+1}),{2*i+1})\n' for i in range(120)))
    checked(['git', 'init', '--quiet', '.'], workspace)
    checked(['git', 'add', 'src', 'arithmetic.py', 'test_arithmetic.py'], workspace)
    checked(['git', '-c', 'core.hooksPath=/dev/null', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
             'commit', '--quiet', '-m', 'public benchmark fixture'], workspace)


def workload(kind):
    if kind == 'compile_test':
        sources = sorted(str(p) for p in Path('src').glob('*.c'))
        checked(['/usr/bin/gcc', '-O2', '-Wall', '-Wextra', '-Werror', *sources, '-o', 'build-result'])
        checked(['./build-result'])
        return {'verified': True, 'translation_units': len(sources), 'binary_sha256': hashlib.sha256(Path('build-result').read_bytes()).hexdigest()}
    if kind == 'repository_edit_test':
        matches = checked(['/usr/bin/git', 'grep', '-n', 'return a-b', '--', 'arithmetic.py'])
        assert 'arithmetic.py' in matches
        Path('arithmetic.py').write_text('def add(a,b):\n    return a+b\n')
        output = subprocess.run(['/usr/bin/python3', '-B', '-m', 'unittest', '-v'], check=True, capture_output=True, text=True, timeout=60)
        assert 'Ran 120 tests' in output.stderr and '\nOK' in output.stderr
        diff = checked(['/usr/bin/git', 'diff', '--', 'arithmetic.py'])
        assert '-    return a-b' in diff and '+    return a+b' in diff
        checked(['/usr/bin/git', 'checkout', '--', 'arithmetic.py'])
        return {'verified': True, 'tests_passed': 120, 'diff': diff}
    if kind == 'artifact_archive':
        archive = 'artifact.tar.gz'
        checked(['/usr/bin/tar', '--sort=name', '--mtime=@0', '--owner=0', '--group=0', '--numeric-owner', '-czf', archive, 'src'])
        destination = Path('unpacked')
        shutil.rmtree(destination, ignore_errors=True); destination.mkdir()
        checked(['/usr/bin/tar', '-xzf', archive, '-C', str(destination)])
        originals = sorted(Path('src').glob('*.c'))
        for source in originals:
            assert source.read_bytes() == (destination / source).read_bytes()
        return {'verified': True, 'roundtrip_files': len(originals), 'archive_sha256': hashlib.sha256(Path(archive).read_bytes()).hexdigest()}
    raise ValueError('unknown workload')


def worker(cpu):
    os.sched_setaffinity(0, {cpu})
    print('ready', flush=True)
    while True:
        request = Path('request.json')
        if not request.exists():
            time.sleep(.01); continue
        job = json.loads(request.read_text()); request.unlink()
        start = time.monotonic()
        result = workload(job['workload'])
        result['wall_seconds'] = time.monotonic() - start
        target = Path(job['output'])
        temporary = target.with_suffix('.tmp'); temporary.write_text(json.dumps(result)); temporary.replace(target)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', type=int)
    parser.add_argument('--binary', type=Path, default=Path('target/release/cerynth-agentd'))
    parser.add_argument('--repeats', type=int, default=5)
    parser.add_argument('--output', type=Path, default=Path('artifacts/person1/tool-workload-comparison.json'))
    args = parser.parse_args()
    if args.worker is not None:
        worker(args.worker); return
    if not 3 <= args.repeats <= 20:
        parser.error('--repeats must be between 3 and 20')
    from harness_runtime_smoke import rpc
    from guard_runtime_smoke import stop_process
    from resource_hierarchy_smoke import wait, admit_when_healthy
    binary = args.binary.resolve()
    cpu = min(os.sched_getaffinity(0))
    with tempfile.TemporaryDirectory(prefix='cerynth-tool-workloads-') as temporary:
        root = Path(temporary); state = root / 'state'
        process = subprocess.Popen([str(binary), '--directory', str(state), 'serve'], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            env=os.environ | {'CERYNTH_RESOURCE_HIERARCHY': '1', 'CERYNTH_PHASE_SCHEDULING': '1',
                              'CERYNTH_AGENT_MEMORY_MAX_BYTES': str(768 * 1024**2)})
        sessions = []
        try:
            wait(lambda: (state / 'agentd.sock').exists())
            def launch(name, code, parent=None):
                workspace = root / name; workspace.mkdir()
                if name == 'foreground':
                    prepare(workspace); shutil.copyfile(__file__, workspace / 'worker.py')
                session = rpc(state, {'type': 'create', 'spec': {'name': name, 'harness': 'generic_pty', 'workspace': str(workspace),
                    'model': {'name': 'none'}, 'runtime': {'parent_id': parent, 'budget': {'memory_max_bytes': (512 if parent is None else 192) * 1024**2,
                    'memory_high_bytes': (512 if parent is None else 192) * 1024**2, 'cpu_quota_micros_per_second': 2_000_000 if parent is None else 1_000_000,
                    'cpu_weight': 100, 'max_subagents': 2 if parent is None else 0, 'tasks_max': 64, 'timeout_seconds': 900,
                    'inference_concurrency': 2 if parent is None else 1}}}})
                assert admit_when_healthy(state, session['id'])['decision'] == 'accept'
                rpc(state, {'type': 'start', 'id': session['id'], 'operation_id': str(uuid.uuid4()), 'launch': {
                    'program': '/usr/bin/python3', 'args': ['-I', '-c', code], 'pty': False, 'initial_input': None}})
                def ready():
                    status = rpc(state, {'type': 'status', 'id': session['id']})
                    if status['state'] in ['failed', 'completed']:
                        raise RuntimeError(rpc(state, {'type': 'output', 'id': session['id'], 'offset': 0, 'limit': 8192}))
                    return 'ready' in rpc(state, {'type': 'output', 'id': session['id'], 'offset': 0, 'limit': 4096})['text']
                wait(ready)
                sessions.append(session)
                return session
            parent = launch('parent', 'import time;print("ready",flush=True);time.sleep(890)')
            foreground = launch('foreground', f'import runpy,sys;sys.argv=["worker.py","--worker","{cpu}"];runpy.run_path("worker.py",run_name="__main__")', parent['id'])
            background = launch('background', f'import os;os.sched_setaffinity(0,{{{cpu}}});data=bytearray(32*1024**2);print("ready",flush=True)\nwhile True: sum(range(10000))', parent['id'])
            for session in [parent, foreground]:
                rpc(state, {'type': 'set_phase', 'id': session['id'], 'phase': 'tool'})
            observations = []
            for repeat in range(args.repeats):
                for condition in (['equal', 'semantic'] if repeat % 2 == 0 else ['semantic', 'equal']):
                    weight = 100 if condition == 'equal' else 40
                    rpc(state, {'type': 'set_phase', 'id': background['id'], 'phase': 'tool' if condition == 'equal' else 'batch'})
                    wait(lambda: rpc(state, {'type': 'resource_accounting', 'id': background['id']})['cpu_weight'] == weight)
                    for kind in ['compile_test', 'repository_edit_test', 'artifact_archive']:
                        before = rpc(state, {'type': 'resource_accounting', 'id': foreground['id']})
                        assert before['cpu_max'] == '100000 100000'
                        name = f'{repeat}-{condition}-{kind}.json'; target = root / 'foreground' / name
                        request = target.parent / 'request.tmp'
                        request.write_text(json.dumps({'workload': kind, 'output': name})); request.replace(target.parent / 'request.json')
                        wait(lambda: target.exists(), timeout=120)
                        result = json.loads(target.read_text()); assert result['verified']
                        after = rpc(state, {'type': 'resource_accounting', 'id': foreground['id']})
                        observations.append({'repeat': repeat, 'condition': condition, 'workload': kind, **result,
                            'cgroup_cpu_usage_usec_delta': after['cpu_stat']['usage_usec'] - before['cpu_stat']['usage_usec'],
                            'memory_current_bytes': after['memory_current_bytes'], 'background_weight': weight})
                        print(json.dumps({k: observations[-1][k] for k in ['repeat', 'condition', 'workload', 'wall_seconds']}), flush=True)
            summaries = {}
            for kind in ['compile_test', 'repository_edit_test', 'artifact_archive']:
                ratios = []
                for repeat in range(args.repeats):
                    pair = {o['condition']: o for o in observations if o['repeat'] == repeat and o['workload'] == kind}
                    ratios.append(pair['semantic']['wall_seconds'] / pair['equal']['wall_seconds'])
                summaries[kind] = {'paired_semantic_over_equal_wall_ratios': ratios, 'mean_paired_ratio': statistics.mean(ratios),
                                   'stdev_paired_ratio': statistics.stdev(ratios)}
            rpc(state, {'type': 'stop', 'id': parent['id']})
            wait(lambda: all(not rpc(state, {'type': 'status', 'id': s['id']})['execution']['resource_allocation']['active'] for s in sessions))
            report = {'schema_version': 1, 'kind': 'managed-real-tool-workload-comparison', 'repeats': args.repeats,
                'runtime_sha256': hashlib.file_digest(binary.open('rb'), 'sha256').hexdigest(),
                'fixture_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                'compiler_version': checked(['/usr/bin/gcc', '--version']).splitlines()[0], 'fixture_cpu': cpu,
                'observations': observations, 'summary': summaries, 'parent_cascade_cleanup': True, 'hard_quotas_unchanged': True,
                'limitations': ['controlled one-CPU tool contention on development host', 'public generated repository, not a model-driven coding task',
                                'warm tools and filesystem cache; other host applications left running', 'Linux default scheduler only',
                                'short workloads may have noisy ratios; no general end-to-end speedup claim']}
            args.output.parent.mkdir(parents=True, exist_ok=True); args.output.write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(summaries))
        finally:
            stop_process(process)


if __name__ == '__main__':
    main()
