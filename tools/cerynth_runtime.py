"""Read-only runtime observation shared by the status and experiment tools."""
import json
import os
import platform
import time
from pathlib import Path


def read_json(path):
    try:
        value = json.loads(Path(path).read_text())
        if not isinstance(value, dict):
            raise ValueError('expected JSON object')
        return value
    except (OSError, ValueError) as error:
        return {'observation_error': str(error)}


class Observer:
    def __init__(self, runtime='/run/cerynth', state=None, proc='/proc', sys='/sys'):
        self.runtime = Path(runtime)
        self.state = Path(state or os.environ.get('CERYNTH_STATE', '/var/lib/cerynth/state.json'))
        self.proc, self.sys = Path(proc), Path(sys)
        self.previous = None
        self.previous_profile = None

    def sample(self):
        now = time.time()
        scheduler = read_json(self.runtime / 'scheduler.json')
        policy = read_json(os.environ.get('CERYNTH_POLICY_PATH', self.runtime / 'policy.json'))
        try:
            policy_age = now - Path(os.environ.get('CERYNTH_POLICY_PATH', self.runtime / 'policy.json')).stat().st_mtime
        except OSError:
            policy_age = None
        stats = read_json(self.runtime / 'scheduler-stats.json')
        state = read_json(self.state)
        try:
            sched_ext = (self.sys / 'kernel/sched_ext/state').read_text().strip()
        except OSError:
            sched_ext = 'unavailable'
        heartbeat = scheduler.get('heartbeat')
        age = now - heartbeat if isinstance(heartbeat, (int, float)) else None
        healthy = (scheduler.get('running') is True and scheduler.get('healthy') is True
                   and sched_ext == 'enabled' and age is not None and 0 <= age <= 5)
        profile = scheduler.get('profile') if healthy else None
        if sched_ext == 'disabled':
            profile = 'linux-default'
        metrics = {}
        try:
            rows = {row.split()[0]: row.split()[1:] for row in (self.proc / 'stat').read_text().splitlines() if row.split()}
            cpu = list(map(int, rows['cpu'][:8]))
            total, idle = sum(cpu), cpu[3] + cpu[4]
            ctxt, churn = int(rows['ctxt'][0]), int(rows['processes'][0])
            current = (time.monotonic(), total, idle, ctxt, churn)
            metrics['runnable_tasks'] = int(rows['procs_running'][0])
            if self.previous:
                before, old_total, old_idle, old_ctxt, old_churn = self.previous
                elapsed = current[0] - before
                delta = total - old_total
                metrics.update(cpu_usage_percent=100 * (1 - (idle-old_idle)/delta) if delta > 0 else None,
                               context_switch_rate=max(0, ctxt-old_ctxt)/elapsed,
                               process_creation_rate=max(0, churn-old_churn)/elapsed)
            self.previous = current
        except (OSError, ValueError, KeyError, IndexError) as error:
            metrics['observation_error'] = str(error)
        result = dict(timestamp_ms=int(now*1000), kernel=platform.release(), sched_ext_state=sched_ext,
                      scheduler_healthy=healthy, heartbeat_age_seconds=age, actual_profile=profile,
                      profile_changed=self.previous_profile is not None and profile != self.previous_profile,
                      adaptation_mode=state.get('adaptation_mode', 'shadow' if state.get('adaptation_enabled') is False else 'unknown'),
                      adaptation_enabled=state.get('adaptation_enabled'), scheduler=scheduler,
                      scheduler_stats=stats, policy=policy, policy_age_seconds=policy_age, telemetry=metrics)
        self.previous_profile = profile
        return result


def render(sample):
    policy, metrics, stats = sample['policy'], sample['telemetry'], sample['scheduler_stats']
    age = sample['heartbeat_age_seconds']
    lines = ['CerynthOS Runtime', '─' * 42,
             f"Kernel:           {sample['kernel']}", f"sched_ext:        {sample['sched_ext_state']}",
             f"Scheduler:        {'healthy' if sample['scheduler_healthy'] else 'unhealthy / unavailable'}",
             f"Profile:          {sample['actual_profile'] or 'unknown'}",
             f"Heartbeat:        {f'{age:.1f}s ago' if age is not None else 'unavailable'}",
             f"Adaptation:       {sample['adaptation_mode'].upper()}",
             f"Recommended:      {policy.get('recommended_profile', 'unavailable')}",
             f"Confidence:       {policy.get('confidence', 'unavailable')}",
             f"Policy age:       {str(round(sample['policy_age_seconds'], 1)) + 's (file mtime)' if sample['policy_age_seconds'] is not None else 'unavailable'}",
             f"Reason:           {policy.get('reason', policy.get('observation_error', 'unavailable'))}"]
    for label, source, key in [('CPU %', metrics, 'cpu_usage_percent'), ('Runnable', metrics, 'runnable_tasks'),
                               ('Ctx switches/s', metrics, 'context_switch_rate'), ('Process churn/s', metrics, 'process_creation_rate'),
                               ('Dispatches', stats, 'tasks_dispatched'), ('Starvation saves', stats, 'starvation_rescues'),
                               ('Dispatch errors', stats, 'dispatch_errors'), ('Dequeue errors', stats, 'dequeue_errors')]:
        value = source.get(key, 'unavailable')
        lines.append(f'{label + ":":18}{value:.2f}' if isinstance(value, float) else f'{label + ":":18}{value}')
    return '\n'.join(lines)
