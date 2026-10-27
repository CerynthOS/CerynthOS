#!/usr/bin/env python3
"""Fail-closed sched_ext burst/profile-transition reproducer; no automatic restart after failure."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
PROFILES = ['balanced','interactive','performance','background','interactive','balanced']


def read_object(path):
    value = json.loads(Path(path).read_text())
    if not isinstance(value, dict):
        raise ValueError(f'{path}: expected object')
    return value


def check_health(profile, runtime=Path('/run/cerynth'), sysroot=Path('/sys'), proc=Path('/proc')):
    status = read_object(runtime/'scheduler.json')
    pid = status.get('pid')
    if not isinstance(pid, int) or pid <= 0:
        raise RuntimeError('missing scheduler PID')
    # Read the actual process, not kill(pid,0), which also accepts zombies.
    stat = (proc/str(pid)/'stat').read_text()
    if stat[stat.find('(')+1:stat.rfind(')')] != 'cerynth-scx':
        raise RuntimeError('scheduler PID belongs to a different process')
    fields = stat[stat.rfind(')')+1:].split()
    if fields[0] in ['Z','X','x']:
        raise RuntimeError('scheduler process exited/zombie')
    if status.get('running') is not True or status.get('healthy') is not True:
        raise RuntimeError('scheduler status reports stopped/unhealthy')
    if str(status.get('profile','')).lower() != profile:
        raise RuntimeError('scheduler profile mismatch')
    stats = read_object(runtime/'scheduler-stats.json')
    if stats.get('pid') != pid:
        raise RuntimeError('stale scheduler stats from another process')
    for name in ['dispatch_errors','dequeue_errors']:
        if stats.get(name,0):
            raise RuntimeError(f'scheduler error counter {name}={stats[name]}')
    # Re-read the kernel state last, and timestamp only after all reads finish.
    state = (sysroot/'kernel/sched_ext/state').read_text().strip()
    now = time.time()
    if state != 'enabled':
        raise RuntimeError(f'sched_ext={state}')
    for name, record in [('status',status),('stats',stats)]:
        heartbeat = record.get('heartbeat')
        # Integer heartbeats are rounded down to seconds by the producer.
        # Test their timestamp interval, rather than adding up to a second
        # to the actual age. Loop progress still has its own 3-second limit.
        quantization = 1 if type(heartbeat) is int else 0
        if (type(heartbeat) not in [int,float] or not 0 <= now-heartbeat
                or now-heartbeat >= 3+quantization):
            raise RuntimeError(f'{name} heartbeat stale/future/missing: now={now}, heartbeat={heartbeat}')
    return dict(timestamp_ms=int(now*1000), scheduler=status, stats=stats, sched_ext=state,
                process_state=fields[0], process_cpu_ticks=int(fields[11])+int(fields[12]))


def check_clean_stop(record, expected_pid):
    if (record.get('pid') != expected_pid or record.get('running') is not False
        or record.get('healthy') is not False or record.get('exit_error')):
        raise RuntimeError('scheduler did not publish a clean stop; refusing reattach')


def watchdog_rows(rows):
    return [row for row in rows if 'sched_ext:' in row.get('MESSAGE','') and any(
        token in row['MESSAGE'] for token in ['runnable task stall','watchdog','failed to run for'])]


class WatchdogFailure(RuntimeError):
    pass


class KernelEvents:
    def __init__(self):
        self.cursor = None
        self.events = []
        self.watchdog_detected = False
        self.unsafe_to_restart = False
        rows = self.query(['-n','1'])
        if rows:
            self.cursor = rows[-1]['__CURSOR']
        else:
            raise RuntimeError('cannot establish kernel journal cursor')

    def query(self, arguments):
        result = subprocess.run(['journalctl','-k','--no-pager','-o','json',*arguments],check=True,
                                capture_output=True,text=True,timeout=10)
        return [json.loads(line) for line in result.stdout.splitlines() if line.strip()]

    def poll(self):
        rows = self.query(['--after-cursor',self.cursor])
        if rows:
            self.cursor = rows[-1]['__CURSOR']
        self.events.extend(row for row in rows if 'sched_ext:' in row.get('MESSAGE',''))
        bad = watchdog_rows(rows)
        if bad:
            self.watchdog_detected = True
            self.unsafe_to_restart = True
            raise WatchdogFailure('kernel watchdog event: '+bad[0]['MESSAGE'])


def terminate(process):
    if process is not None and process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def run_linux_baseline(kernel, command, directory, poll=.2,
                       state_path=Path('/sys/kernel/sched_ext/state')):
    """Finish load under Linux before the next cold attachment; never reattach here."""
    if state_path.read_text().strip() != 'disabled':
        raise RuntimeError('Linux baseline requires sched_ext disabled')
    kernel.poll()
    directory.mkdir()
    with (directory/'workload.log').open('w') as log, (directory/'observations.jsonl').open('w') as output:
        process = subprocess.Popen(command,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
        deadline = time.monotonic()+600
        try:
            while True:
                state = state_path.read_text().strip()
                if state != 'disabled':
                    raise RuntimeError('scheduler unexpectedly enabled during Linux baseline')
                kernel.poll()
                output.write(json.dumps(dict(timestamp_ms=int(time.time()*1000),sched_ext=state))+'\n')
                output.flush()
                if process.poll() is not None:
                    if process.returncode:
                        raise RuntimeError(f'Linux baseline exited {process.returncode}')
                    return dict(command=command,exit_code=0,directory=str(directory))
                if time.monotonic() >= deadline:
                    raise RuntimeError('Linux baseline safety timeout')
                time.sleep(poll)
        finally:
            terminate(process)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--ctl', default=str(ROOT/'tools/validation-ctl'))
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--duration', type=float, default=15)
    parser.add_argument('--cpu-ms', type=float, default=20)
    parser.add_argument('--workers', type=int, default=32)
    parser.add_argument('--idle-soak', type=float, default=10)
    parser.add_argument('--linux-baseline', type=int, default=0, help='CPU-load seconds under Linux before each round cold-start')
    parser.add_argument('--poll', type=float, default=.2)
    args = parser.parse_args()
    if args.rounds < 1 or args.duration <= 0 or args.workers < 1 or args.workers > 256 or args.cpu_ms <= 0 or args.poll <= 0 or args.idle_soak < 0 or args.linux_baseline < 0:
        parser.error('positive parameters required')
    if args.output.exists():
        parser.error('output already exists; use a new result directory')
    args.output.mkdir(parents=True)
    kernel = KernelEvents()
    process = None
    completed = []
    baselines = []
    current = None
    def save(name, value):
        (args.output/name).write_text(json.dumps(value,indent=2)+'\n')
    def ctl(*words):
        result = subprocess.run([args.ctl,*words],capture_output=True,text=True,timeout=30)
        if result.returncode or '✗' in result.stdout+result.stderr:
            raise RuntimeError(f'control {words}: {result.stdout}{result.stderr}')
        kernel.poll() # Never start the next profile after a watchdog on stop.
    binary_hash = hashlib.sha256(Path('/usr/lib/cerynth/cerynth-scx').read_bytes()).hexdigest()
    parameters = dict(rounds=args.rounds,duration=args.duration,workers=args.workers,cpu_ms=args.cpu_ms,
                      inter_burst_idle_seconds=.1,barrier_timeout_seconds=4,poll_seconds=args.poll,
                      idle_soak=args.idle_soak,linux_baseline=args.linux_baseline,
                      profiles=PROFILES,kernel=os.uname().release,initial_cursor=kernel.cursor,
                      scheduler_sha256=binary_hash)
    save('parameters.json',parameters)
    try:
        for number in range(args.rounds):
            for profile_index, profile in enumerate(PROFILES):
                current = dict(round=number+1,profile=profile)
                print(current,flush=True)
                phase = args.output/f'{len(completed)+1:03}-{profile}'
                phase.mkdir()
                kernel.poll()
                state = Path('/sys/kernel/sched_ext/state').read_text().strip()
                if state == 'enabled':
                    stopped_pid = read_object(Path('/run/cerynth/scheduler.json'))['pid']
                    ctl('stop')
                    check_clean_stop(read_object(Path('/run/cerynth/scheduler.json')), stopped_pid)
                elif state == 'disabled' and completed:
                    raise RuntimeError('unexpected Linux fallback between phases; refusing reattach')
                elif state != 'disabled':
                    raise RuntimeError(f'cannot transition from sched_ext={state}')
                if Path('/sys/kernel/sched_ext/state').read_text().strip() != 'disabled':
                    raise RuntimeError('stop did not reach Linux fallback')
                if profile_index == 0 and args.linux_baseline:
                    baseline = run_linux_baseline(kernel,
                        ['bash',str(ROOT/'tests/workloads/cpu-stress.sh'),str(args.linux_baseline)],
                        args.output/f'linux-before-round-{number+1}',args.poll)
                    baselines.append(baseline)
                    save('linux-baselines.json',baselines)
                ctl('profile','set',profile)
                ctl('start')
                deadline = time.monotonic()+5
                while True:
                    kernel.poll()
                    try:
                        first = check_health(profile)
                        break
                    except (OSError, ValueError, RuntimeError):
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(.05)
                expected_pid = first['scheduler']['pid']
                progress = phase/'worker-progress.json'
                with (phase/'workload.log').open('w') as log, (phase/'observations.jsonl').open('w') as observations:
                    process = subprocess.Popen([sys.executable,str(ROOT/'tests/workloads/scheduler-burst.py'),
                        '--duration',str(args.duration),'--workers',str(args.workers),'--cpu-ms',str(args.cpu_ms),'--output',str(progress)],
                        stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
                    last_worker_change = time.monotonic()
                    last_count = 0
                    last_journal = time.monotonic()
                    last_sequence = first['stats']['progress_sequence']
                    last_scheduler_change = time.monotonic()
                    while True:
                        snapshot = check_health(profile)
                        if snapshot['scheduler']['pid'] != expected_pid:
                            raise RuntimeError('scheduler PID changed during workload')
                        sequence = snapshot['stats']['progress_sequence']
                        if sequence > last_sequence:
                            last_scheduler_change = time.monotonic()
                            last_sequence = sequence
                        if time.monotonic()-last_scheduler_change > 3:
                            raise RuntimeError('scheduler loop stopped making progress')
                        if progress.exists():
                            workers = read_object(progress)
                            snapshot['worker_progress'] = workers
                            count = workers['completed_bursts']
                            if count > last_count:
                                last_worker_change = time.monotonic()
                                last_count = count
                        if time.monotonic()-last_worker_change > 4:
                            raise RuntimeError('burst workers stopped making progress')
                        observations.write(json.dumps(snapshot)+'\n')
                        observations.flush()
                        if time.monotonic()-last_journal >= 1:
                            kernel.poll()
                            last_journal = time.monotonic()
                        if process.poll() is not None:
                            if process.returncode:
                                raise RuntimeError(f'burst workload exited {process.returncode}')
                            final = read_object(progress)
                            if not final['completed'] or not final['completed_bursts'] or any(
                                count != final['completed_bursts'] for count in final['worker_completions']):
                                raise RuntimeError('not all runnable workers completed')
                            break
                        time.sleep(args.poll)
                kernel.poll()
                check_health(profile)
                completed.append(dict(**current,worker_bursts=last_count,pid=expected_pid))
                save('completed.json',completed)
        idle_end = time.monotonic()+args.idle_soak
        last_progress = time.monotonic()
        sequence = check_health('balanced')['stats']['progress_sequence']
        with (args.output/'idle-observations.jsonl').open('w') as output:
            while time.monotonic() < idle_end:
                kernel.poll()
                snapshot = check_health('balanced')
                if snapshot['stats']['progress_sequence'] > sequence:
                    sequence = snapshot['stats']['progress_sequence']
                    last_progress = time.monotonic()
                if time.monotonic()-last_progress > 3:
                    raise RuntimeError('scheduler stopped progressing during idle soak')
                output.write(json.dumps(snapshot)+'\n')
                output.flush()
                time.sleep(args.poll)
        kernel.poll()
        save('result.json',dict(status='pass',completed_phases=len(completed),completed_linux_baselines=len(baselines),watchdog_events=[],parameters=parameters))
    except BaseException as error:
        # Capture first. Do not start, switch profile, or otherwise reattach.
        try:
            kernel.poll()
        except Exception:
            pass
        save('failure.json',dict(error=str(error),phase=current,completed_phases=len(completed)))
        for name in ['scheduler.json','scheduler-stats.json','policy.json']:
            try:
                (args.output/('failure-'+name)).write_bytes((Path('/run/cerynth')/name).read_bytes())
            except OSError:
                pass
        save('kernel-events.json',kernel.events)
        raise
    finally:
        terminate(process)
        save('kernel-events.json',kernel.events)


if __name__ == '__main__':
    def interrupted(signum, frame):
        raise InterruptedError(f'validation interrupted by signal {signum}')
    signal.signal(signal.SIGTERM,interrupted)
    try:
        main()
    except (RuntimeError,OSError,ValueError,subprocess.SubprocessError) as error:
        sys.exit(str(error))
