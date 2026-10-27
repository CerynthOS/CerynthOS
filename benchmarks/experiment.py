#!/usr/bin/env python3
"""Benchmark matrix and adaptive experiment runner (Linux, Python standard library)."""
import argparse
import hashlib
import importlib.util
import itertools
import json
import math
import os
from pathlib import Path
import platform
import signal
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools'))
from cerynth_runtime import Observer, read_json

PROFILES = ['linux-default', 'balanced', 'interactive', 'performance', 'background']
WORKLOADS = ['idle', 'cpu-stress', 'interactive-stress', 'burst', 'compile', 'mixed']
SEQUENCE = [('idle', 30), ('interactive-stress', 60), ('cpu-stress', 60), ('mixed', 60), ('idle', 30)]


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def terminate(process):
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def workload_command(name, duration):
    if name == 'mixed':
        return ['bash', str(ROOT / 'benchmarks/mixed.sh'), str(duration)]
    name = 'idle-baseline' if name == 'idle' else name
    return ['bash', str(ROOT / 'tests/workloads' / (name + '.sh')), str(duration)]


def verify(sample, profile):
    actual = sample['actual_profile']
    if not actual or actual.lower() != profile or (profile != 'linux-default' and not sample['scheduler_healthy']):
        raise RuntimeError(f"requested {profile}, observed {actual}; sched_ext={sample['sched_ext_state']}")


def run_workload(observer, directory, workload, duration, interval, profile=None, audit=None, kernel=None):
    directory.mkdir(parents=True)
    samples = []
    observer.sample()  # Exclude scheduler-switch activity from workload CPU/rate deltas.
    started = time.time()
    audit_offset = audit.stat().st_size if audit and audit.exists() else 0
    audit_inode = audit.stat().st_ino if audit and audit.exists() else None
    audit_events = []
    def collect_audit():
        nonlocal audit_offset, audit_inode
        if audit and audit.exists():
            st = audit.stat()
            if st.st_ino != audit_inode or st.st_size < audit_offset:
                audit_offset = 0
            audit_inode = st.st_ino
            with audit.open('rb') as source:
                source.seek(audit_offset)
                for line in source:
                    if not line.endswith(b'\n'):
                        break
                    audit_offset += len(line)
                    try:
                        audit_events.append(json.loads(line))
                    except ValueError:
                        audit_events.append({'observation_error': 'malformed audit line', 'raw': line.decode(errors='replace')})
    with (directory / 'workload.log').open('w') as log, (directory / 'observations.jsonl').open('w') as output:
        process = subprocess.Popen(workload_command(workload, duration), stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            time.sleep(interval)
            while True:
                sample = observer.sample()
                samples.append(sample)
                output.write(json.dumps(sample) + '\n')
                output.flush()
                collect_audit()
                if kernel:
                    kernel.poll()
                    if not sample['scheduler_healthy'] and profile != 'linux-default':
                        kernel.unsafe_to_restart = True
                        raise RuntimeError('scheduler health lost during gated experiment')
                if profile:
                    verify(sample, profile)
                if process.poll() is not None:
                    break
                if time.time() - started > max(duration * 10, 600):
                    raise RuntimeError('workload exceeded safety timeout')
                time.sleep(interval)
            if process.returncode:
                raise RuntimeError(f'{workload} exited {process.returncode}; see {directory}/workload.log')
        finally:
            terminate(process)
            collect_audit()
            (directory / 'adaptation-audit.jsonl').write_text(''.join(json.dumps(event)+'\n' for event in audit_events))
    summary = {'profile': profile, 'workload': workload, 'sample_count': len(samples),
               'elapsed_seconds': time.time()-started, 'profile_changes': sum(s['profile_changed'] for s in samples),
               'audit_available': bool(audit and audit.exists()),
               'audit_events': len(audit_events) if audit and audit.exists() else None,
               'rollback_events': sum(bool(e.get('rollback')) for e in audit_events) if audit and audit.exists() else None}
    for key in ['cpu_usage_percent', 'context_switch_rate', 'process_creation_rate', 'runnable_tasks']:
        values = [s['telemetry'][key] for s in samples if isinstance(s['telemetry'].get(key), (int, float))]
        if not values:
            raise RuntimeError(f'no telemetry for {key}')
        summary['mean_' + key] = statistics.mean(values)
    summary['workload_metrics'] = {}
    for line in (directory / 'workload.log').read_text().splitlines():
        if '=' in line:
            key, value = line.strip().split('=', 1)
            if key in ['interactive_iterations', 'completed_bursts']:
                summary['workload_metrics'][key] = int(value)
        parts = line.split()
        if 'cpu' in parts:
            values = parts[parts.index('cpu') + 1:]
            if len(values) == 6:
                try:
                    summary['workload_metrics']['stress_ng_bogo_ops'] = int(values[0])
                    summary['workload_metrics']['stress_ng_bogo_ops_per_second'] = float(values[4])
                except ValueError:
                    pass
    # Additional workload metrics remain inspectable in the raw log.
    write_json(directory / 'summary.json', summary)
    write_json(directory / 'metadata.json', {'kernel': platform.release(), 'cpu_count': os.cpu_count(),
               'started_at_ms': int(started*1000), 'requested_duration_seconds': duration, 'interval_seconds': interval,
               'command': workload_command(workload, duration), 'workload_exit': process.returncode})
    return summary


def aggregate(output, results, repeats):
    lines = ['# CerynthOS benchmark matrix', '', 'Means and sample standard deviations across independent runs; CPU use is not a throughput score.', '',
             '| Workload | Profile | Runs | CPU % (mean ± SD) | Context switches/s | Process creations/s | Elapsed seconds | Workload metrics |', '|---|---|---:|---:|---:|---:|---:|---|']
    summaries = output / 'summaries'
    summaries.mkdir(exist_ok=True)
    for workload, profile in itertools.product(WORKLOADS, PROFILES):
        group = [r for r in results if r['workload'] == workload and r['profile'] == profile]
        if len(group) < repeats:
            raise RuntimeError(f'incomplete combination: {workload}/{profile}')
        result = {'profile': profile, 'workload': workload, 'runs': len(group),
                  'measurement_origins': [r.get('measurement_origin', 'original session') for r in group]}
        for key in ['mean_cpu_usage_percent', 'mean_context_switch_rate', 'mean_process_creation_rate', 'elapsed_seconds']:
            values = [r[key] for r in group]
            result[key] = {'mean': statistics.mean(values), 'stdev': statistics.stdev(values), 'min': min(values), 'max': max(values)}
        result['workload_metrics'] = {}
        metric_keys = set().union(*(r.get('workload_metrics', {}).keys() for r in group))
        for key in sorted(metric_keys):
            values = [r['workload_metrics'][key] for r in group if key in r.get('workload_metrics', {})]
            if len(values) == len(group):
                result['workload_metrics'][key] = {'mean': statistics.mean(values), 'stdev': statistics.stdev(values)}
        write_json(summaries / f'{profile}-{workload}.json', result)
        cpu = result['mean_cpu_usage_percent']
        workload_metrics = '; '.join(f"{key}: {value['mean']:.2f} ± {value['stdev']:.2f}" for key, value in result['workload_metrics'].items()) or 'see raw log'
        lines.append(f"| {workload} | {profile} | {len(group)} | {cpu['mean']:.2f} ± {cpu['stdev']:.2f} | {result['mean_context_switch_rate']['mean']:.2f} | {result['mean_process_creation_rate']['mean']:.2f} | {result['elapsed_seconds']['mean']:.2f} | {workload_metrics} |")
    reused = sum(r.get('measurement_origin') == 'reused before this session' for r in results)
    lines += ['', f'Measurement provenance: {reused} runs reused from before this session; {len(results)-reused} newly measured runs. See measurement-session-*.json for separate path lists and per-combination measurement_origins in summaries.', '', 'Raw observations, scheduler stats, policy reasons and workload logs are in `raw/`.',
              'Compile duration is advisory: each clean compilation finishes before sampling ends.',
              'Use workload logs and elapsed time alongside system metrics; no latency or throughput advantage is inferred automatically.']
    (output / 'report.md').write_text('\n'.join(lines)+'\n')


def completed_run(directory, item, interval=1):
    """Resume only from a successful run with matching raw health observations."""
    summary = json.loads((directory / 'summary.json').read_text())
    metadata = json.loads((directory / 'metadata.json').read_text())
    samples = [json.loads(line) for line in (directory / 'observations.jsonl').read_text().splitlines() if line.strip()]
    if (summary.get('profile') != item['profile'] or summary.get('workload') != item['workload']
        or metadata.get('workload_exit') != 0 or metadata.get('requested_duration_seconds') != item['duration']
        or metadata.get('interval_seconds', 1) != interval
        or len(samples) != summary.get('sample_count') or not samples):
        raise RuntimeError(f'invalid completed run: {directory}')
    for sample in samples:
        verify(sample, item['profile'])
    return summary


def verify_stability_gates(paths, scheduler_binary):
    if len(paths) != 2 or len({p.resolve() for p in paths}) != 2:
        raise RuntimeError('two distinct smoke and sustained stability results are required')
    digest = hashlib.sha256(Path(scheduler_binary).read_bytes()).hexdigest()
    records = [json.loads(path.read_text()) for path in paths]
    for record in records:
        params = record.get('parameters', {})
        if (record.get('status') != 'pass' or record.get('watchdog_events') != []
            or params.get('scheduler_sha256') != digest or params.get('kernel') != platform.release()
            or params.get('workers',0) < 32
            or record.get('completed_phases', 0) < 6):
            raise RuntimeError('stability result failed, incomplete, or belongs to a different binary/kernel')
    if not any(r['parameters'].get('rounds',0) >= 3 and r['parameters'].get('duration',0) >= 15
               and r['parameters'].get('idle_soak',0) >= 120
               and r['parameters'].get('linux_baseline',0) >= 30
               and r.get('completed_linux_baselines',0) >= 3 for r in records):
        raise RuntimeError('sustained multi-profile burst and idle stability result is required')
    return records


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['matrix', 'adaptive'])
    parser.add_argument('--output', type=Path)
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--duration', type=int, default=30)
    parser.add_argument('--interval', type=float, default=1)
    parser.add_argument('--runtime-dir', default='/run/cerynth')
    parser.add_argument('--state', default=os.environ.get('CERYNTH_STATE', '/var/lib/cerynth/state.json'))
    parser.add_argument('--ctl', default='cerynthctl')
    parser.add_argument('--audit', type=Path, default=Path('/var/log/cerynth/adaptation.jsonl'))
    parser.add_argument('--reuse-from', type=Path, help='read successful runs from an unchanged earlier matrix; write only to a new output')
    parser.add_argument('--resume', action='store_true', help='reuse verified successful matrix runs after an interrupted experiment')
    parser.add_argument('--stability-gates', type=Path, nargs='+')
    parser.add_argument('--scheduler-binary', default='/usr/lib/cerynth/cerynth-scx')
    parser.add_argument('--plan', action='store_true', help='print the complete run plan without executing')
    args = parser.parse_args()
    if args.repeats < 3 or args.duration <= 0 or not math.isfinite(args.interval) or args.interval <= 0:
        parser.error('require repeats >= 3, duration > 0 and finite interval > 0')
    plan = ([dict(profile=p, workload=w, repeat=r, duration=args.duration)
             for r in range(1, args.repeats+1) for w in WORKLOADS
             for p in PROFILES[(r-1)%5:] + PROFILES[:(r-1)%5]] if args.mode == 'matrix'
            else [dict(workload=w, duration=d) for w, d in SEQUENCE])
    if args.plan:
        print(json.dumps(plan, indent=2))
        return
    if platform.system() != 'Linux':
        parser.error('live experiments require Linux with /proc and sched_ext')
    output = args.output or ROOT / 'artifacts' / ('benchmark-matrix' if args.mode == 'matrix' else 'adaptive-evaluation')
    if args.resume and args.reuse_from:
        parser.error('--resume and --reuse-from cannot be combined')
    if args.reuse_from and args.mode != 'matrix':
        parser.error('--reuse-from is supported for the matrix only')
    if args.reuse_from and output.resolve().is_relative_to(args.reuse_from.resolve()):
        parser.error('--reuse-from requires a different, new output directory')
    if args.reuse_from:
        if json.loads((args.reuse_from / 'plan.json').read_text()) != plan:
            parser.error('reuse requires the original, identical run plan')
    if args.resume and args.mode != 'matrix':
        parser.error('--resume is supported for the matrix only')
    if output.exists() and not args.resume:
        parser.error(f'{output} already exists; choose a new --output to preserve earlier runs')
    if args.resume:
        if not (output / 'plan.json').exists() or json.loads((output / 'plan.json').read_text()) != plan:
            parser.error('resume requires the original, identical run plan')
    kernel = None
    if args.stability_gates:
        verify_stability_gates(args.stability_gates, args.scheduler_binary)
        spec = importlib.util.spec_from_file_location('cerynth_stability', ROOT/'tools/cerynth-stability.py')
        stability = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(stability)
        kernel = stability.KernelEvents()
    observer = Observer(args.runtime_dir, args.state)
    observer.sample()
    time.sleep(args.interval)
    original = observer.sample()
    def ctl(*words):
        result = subprocess.run([args.ctl, *words], check=True, timeout=30, capture_output=True, text=True)
        # Current CLI prints daemon errors but may still exit zero.
        if '✗' in result.stderr or '✗' in result.stdout:
            raise RuntimeError(result.stderr or result.stdout)
        print(result.stdout, end='')
        if kernel:
            kernel.poll()
    if args.mode == 'matrix':
        state = read_json(args.state)
        if state.get('scheduler_backend') != 'scx':
            parser.error('matrix requires scheduler_backend=scx in runtime state')
        if state.get('adaptation_enabled') is not False:
            parser.error('pause adaptation before matrix measurements; state must explicitly report adaptation_enabled=false')
        if original['actual_profile'] not in ['linux-default', *[p.title() for p in PROFILES[1:]], *PROFILES[1:]]:
            parser.error('cannot determine original scheduler/profile for restoration')
    def stop_scheduler():
        sample = observer.sample()
        if sample['sched_ext_state'] != 'disabled':
            stopped_pid = sample['scheduler'].get('pid')
            ctl('stop')
            if kernel:
                try:
                    stability.check_clean_stop(read_json(Path(args.runtime_dir)/'scheduler.json'), stopped_pid)
                except RuntimeError:
                    kernel.unsafe_to_restart = True
                    raise
    output.mkdir(parents=True, exist_ok=args.resume)
    if args.resume and (output / 'failure.json').exists():
        (output / 'failure.json').rename(output / f'failure-before-resume-{time.time_ns()}.json')
    write_json(output / 'plan.json', plan)
    results = []
    session_path = output / f'measurement-session-{time.time_ns()}.json'
    session = dict(started_at_ms=int(time.time()*1000), resumed=args.resume, reuse_from=str(args.reuse_from) if args.reuse_from else None, reused_runs=[], new_runs=[])
    write_json(session_path, session)
    try:
        for index, item in enumerate(plan):
            print(f'[{index+1}/{len(plan)}] {item}', flush=True)
            if args.mode == 'matrix':
                profile = item['profile']
                directory = output / 'raw' / profile / item['workload'] / str(item['repeat'])
                prior = (args.reuse_from / 'raw' / profile / item['workload'] / str(item['repeat'])) if args.reuse_from else directory
                if (args.resume or args.reuse_from) and (prior / 'summary.json').exists():
                    summary = completed_run(prior, item, args.interval)
                    summary['measurement_origin'] = 'reused before this session'
                    results.append(summary)
                    session['reused_runs'].append(str(prior.resolve()))
                    write_json(session_path, session)
                    print(f'Reused verified run: {prior}', flush=True)
                    continue
                if args.resume and directory.exists():
                    failed = output / 'failed-attempts'
                    failed.mkdir(exist_ok=True)
                    directory.rename(failed / f'{profile}-{item["workload"]}-{item["repeat"]}-{time.time_ns()}')
                if profile == 'linux-default':
                    stop_scheduler()
                else:
                    stop_scheduler()
                    ctl('profile', 'set', profile)
                    ctl('start')
                deadline = time.monotonic() + 10
                while True:
                    try:
                        verify(observer.sample(), profile)
                        break
                    except RuntimeError:
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(.2)
                directory = output / 'raw' / profile / item['workload'] / str(item['repeat'])
            else:
                profile = None
                directory = output / 'raw' / f'{index+1:02}-{item["workload"]}'
            summary = run_workload(observer, directory, item['workload'], item['duration'], args.interval, profile, args.audit, kernel) if kernel else run_workload(observer, directory, item['workload'], item['duration'], args.interval, profile, args.audit)
            summary['measurement_origin'] = session_path.stem
            write_json(directory / 'summary.json', summary)
            results.append(summary)
            session['new_runs'].append(str(directory.relative_to(output)))
            write_json(session_path, session)
        if args.mode == 'matrix':
            aggregate(output, results, args.repeats)
        else:
            write_json(output / 'summary.json', results)
            phase_table = '\n| Phase | Seconds | Profile changes | Rollback events |\n|---|---:|---:|---:|\n' + ''.join(
                f"| {r['workload']} | {r['elapsed_seconds']:.1f} | {r['profile_changes']} | {r['rollback_events'] if r['audit_available'] else 'unavailable'} |\n" for r in results)
            (output / 'report.md').write_text('# Adaptive evaluation\n\nSequence: idle 30s → interactive 60s → CPU 60s → mixed 60s → idle 30s.\n\n'
                'See raw/*/observations.jsonl for recommendations, actual profiles, transitions, confidence, reasons, scheduler stats and telemetry. '
                'Each phase includes adaptation-audit.jsonl with events observed during that phase. '
                'This runner preserves the configured adaptation mode; canary must be explicitly enabled beforehand. '
                'Missing stats or audit files are unavailable evidence, never proof of zero errors or rollbacks.\n' + phase_table)
        write_json(output/'completion.json', dict(status='pass', completed_runs=len(results), mode=args.mode))
    except BaseException as error:
        if kernel:
            kernel.unsafe_to_restart = True
        write_json(output / 'failure.json', {'error': str(error), 'completed_runs': len(results)})
        raise
    finally:
        if kernel:
            write_json(output/'kernel-events.json', kernel.events)
        if kernel and observer.sample()['sched_ext_state'] == 'disabled':
            record = read_json(Path(args.runtime_dir)/'scheduler.json')
            try:
                stability.check_clean_stop(record, record.get('pid'))
            except RuntimeError:
                kernel.unsafe_to_restart = True
        if args.mode == 'matrix' and (not kernel or not kernel.unsafe_to_restart):
            if kernel:
                kernel.poll()
            stop_scheduler()
            restore_profile = state.get('profile') if original['actual_profile'] == 'linux-default' else original['actual_profile'].lower()
            if restore_profile:
                ctl('profile', 'set', restore_profile.lower())
            if original['actual_profile'] != 'linux-default':
                ctl('start')


if __name__ == '__main__':
    def interrupted(signum, frame):
        raise InterruptedError(f'validation interrupted by signal {signum}')
    signal.signal(signal.SIGTERM, interrupted)
    try:
        main()
    except (RuntimeError, OSError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
