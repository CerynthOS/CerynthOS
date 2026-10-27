import importlib.util
import json
from pathlib import Path
import tempfile
import os
import subprocess
import time
import unittest
from unittest.mock import patch, Mock

from cerynth_runtime import Observer, render

spec = importlib.util.spec_from_file_location('experiment', Path(__file__).resolve().parents[1] / 'benchmarks/experiment.py')
experiment = importlib.util.module_from_spec(spec)
spec.loader.exec_module(experiment)


class ObservabilityTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / 'kernel/sched_ext').mkdir(parents=True)
        (self.root / 'kernel/sched_ext/state').write_text('enabled')
        (self.root / 'stat').write_text('cpu 10 0 10 80 0 0 0 0\nctxt 100\nprocesses 20\nprocs_running 2\n')
        self.put('scheduler.json', dict(running=True, healthy=True, profile='Interactive', heartbeat=time.time()))
        self.put('policy.json', dict(recommended_profile='performance', confidence=.84, reason='CPU saturation'))
        self.put('state.json', dict(adaptation_enabled=False))
        self.observer = Observer(self.root, self.root / 'state.json', self.root, self.root)

    def put(self, name, value):
        (self.root / name).write_text(json.dumps(value))

    def test_health_and_stale_heartbeat(self):
        sample = self.observer.sample()
        self.assertTrue(sample['scheduler_healthy'])
        self.assertIn('CPU saturation', render(sample))
        self.assertIn('unavailable', render(sample))
        self.put('scheduler.json', dict(running=True, healthy=True, profile='Interactive', heartbeat=time.time()-10))
        self.assertFalse(self.observer.sample()['scheduler_healthy'])
        with self.assertRaises(RuntimeError):
            experiment.verify(self.observer.sample(), 'interactive')

    def test_linux_fallback_and_corruption(self):
        (self.root / 'kernel/sched_ext/state').write_text('disabled')
        (self.root / 'policy.json').write_text('{')
        sample = self.observer.sample()
        self.assertEqual(sample['actual_profile'], 'linux-default')
        self.assertIn('observation_error', sample['policy'])
        experiment.verify(sample, 'linux-default')

    def test_cpu_and_churn_deltas(self):
        self.observer.sample()
        (self.root / 'stat').write_text('cpu 30 0 30 140 0 0 0 0\nctxt 200\nprocesses 25\nprocs_running 4\n')
        sample = self.observer.sample()
        self.assertAlmostEqual(sample['telemetry']['cpu_usage_percent'], 40)
        self.assertGreater(sample['telemetry']['process_creation_rate'], 0)

    def test_run_failure_preserves_raw_evidence(self):
        self.observer.sample()
        with patch.object(experiment, 'workload_command', return_value=['sh', '-c', 'echo failed; exit 7']):
            with self.assertRaises(RuntimeError):
                experiment.run_workload(self.observer, self.root / 'run', 'idle', 1, .01)
        self.assertIn('failed', (self.root / 'run/workload.log').read_text())
        self.assertTrue((self.root / 'run/observations.jsonl').exists())
        self.assertFalse((self.root / 'run/summary.json').exists())

    def test_failure_restores_original_profile(self):
        self.put('state.json', dict(adaptation_enabled=False, scheduler_backend='scx', profile='interactive'))
        output = self.root / 'matrix'
        with patch.object(experiment.sys, 'argv', ['experiment', 'matrix', '--runtime-dir', str(self.root),
                          '--state', str(self.root / 'state.json'), '--output', str(output)]), \
             patch.object(experiment.platform, 'system', return_value='Linux'), \
             patch.object(experiment.time, 'sleep'), \
             patch.object(experiment, 'Observer', return_value=self.observer), \
             patch.object(experiment, 'verify'), \
             patch.object(experiment, 'run_workload', side_effect=RuntimeError('injected failure')), \
             patch.object(experiment.subprocess, 'run', return_value=Mock(stdout='', stderr='')) as ctl:
            with self.assertRaisesRegex(RuntimeError, 'injected failure'):
                experiment.main()
        self.assertTrue((output / 'failure.json').exists())
        self.assertFalse((output / 'report.md').exists())
        self.assertEqual([c.args[0] for c in ctl.call_args_list[-3:]],
                         [['cerynthctl', 'stop'], ['cerynthctl', 'profile', 'set', 'interactive'], ['cerynthctl', 'start']])

    def test_resume_checks_raw_health_and_run_metadata(self):
        item = dict(profile='interactive', workload='idle', duration=30)
        self.put('summary.json', dict(profile='interactive', workload='idle', sample_count=1))
        self.put('metadata.json', dict(workload_exit=0, requested_duration_seconds=30))
        raw = self.root / 'observations.jsonl'
        raw.write_text(json.dumps(self.observer.sample()) + '\n')
        self.assertEqual(experiment.completed_run(self.root, item)['sample_count'], 1)
        broken = self.observer.sample()
        broken['scheduler_healthy'] = False
        raw.write_text(json.dumps(broken) + '\n')
        with self.assertRaises(RuntimeError):
            experiment.completed_run(self.root, item)
        raw.write_text(json.dumps(self.observer.sample()) + '\n')
        self.put('metadata.json', dict(workload_exit=7, requested_duration_seconds=30))
        with self.assertRaises(RuntimeError):
            experiment.completed_run(self.root, item)

    def test_read_only_reuse_keeps_original_tree_unchanged(self):
        from validation_manifest import manifest
        prior = self.root / 'prior'
        directory = prior / 'raw/interactive/idle/1'
        directory.mkdir(parents=True)
        summary = dict(profile='interactive', workload='idle', sample_count=1, elapsed_seconds=1,
                       mean_cpu_usage_percent=1, mean_context_switch_rate=1, mean_process_creation_rate=1)
        (directory / 'summary.json').write_text(json.dumps(summary))
        (directory / 'metadata.json').write_text(json.dumps(dict(workload_exit=0, requested_duration_seconds=30)))
        (directory / 'observations.jsonl').write_text(json.dumps(self.observer.sample())+'\n')
        plan = [dict(profile=p, workload='idle', repeat=r, duration=30) for p,r in
                [('linux-default',1),('interactive',1),('interactive',2),('linux-default',2),('linux-default',3),('interactive',3)]]
        (prior / 'plan.json').write_text(json.dumps(plan))
        (prior / 'failure.json').write_text('{"original_failure":true}')
        original = manifest(prior)
        output = self.root / 'new'
        self.put('state.json', dict(adaptation_enabled=False, scheduler_backend='scx', profile='interactive'))
        def new_run(observer, path, workload, duration, interval, profile, audit):
            path.mkdir(parents=True)
            return {**summary, 'profile':profile, 'elapsed_seconds': int(path.name)}
        with patch.object(experiment.sys, 'argv', ['experiment','matrix','--reuse-from',str(prior),
                          '--output',str(output),'--state',str(self.root/'state.json')]), \
             patch.object(experiment.platform, 'system', return_value='Linux'), \
             patch.object(experiment.time, 'sleep'), \
             patch.object(experiment, 'Observer', return_value=self.observer), \
             patch.object(experiment, 'PROFILES', ['linux-default','interactive']), \
             patch.object(experiment, 'verify'), \
             patch.object(experiment, 'WORKLOADS', ['idle']), \
             patch.object(experiment.subprocess, 'run', return_value=Mock(stdout='',stderr='')), \
             patch.object(experiment, 'run_workload', side_effect=new_run):
            experiment.main()
        self.assertEqual(original, manifest(prior))
        self.assertFalse((output/'raw/interactive/idle/1').exists())
        session=json.loads(next(output.glob('measurement-session-*.json')).read_text())
        self.assertEqual(len(session['reused_runs']),1)
        self.assertEqual(len(session['new_runs']),5)
        self.assertTrue((output/'report.md').exists())

    def test_control_wrapper_rejects_extra_arguments(self):
        wrapper=Path(__file__).with_name('validation-ctl')
        mock_sudo=self.root/'sudo'
        log=self.root/'sudo-args.json'
        mock_sudo.write_text('#!/usr/bin/env python3\nimport json,os,sys\nfrom pathlib import Path\nPath(os.environ["TEST_SUDO_LOG"]).write_text(json.dumps(sys.argv[1:]))\n')
        mock_sudo.chmod(0o755)
        env={**os.environ,'PATH':str(self.root)+os.pathsep+os.environ['PATH'],'TEST_SUDO_LOG':str(log)}
        for args in [['start','extra'],['stop','extra'],['start',''],['status','extra'],
                     ['start ignored'],['profile set balanced'],['profile','set','balanced','extra'],
                     ['profile','set','balanced --help']]:
            result=subprocess.run(['bash',str(wrapper),*args],env=env,capture_output=True)
            self.assertEqual(result.returncode,64,args)
            self.assertFalse(log.exists(),args)
        for args in [['start'],['stop'],['status'],['profile','set','balanced'],
                     ['profile','set','interactive'],['profile','set','performance'],['profile','set','background']]:
            result=subprocess.run(['bash',str(wrapper),*args],env=env,capture_output=True)
            self.assertEqual(result.returncode,0,args)
            self.assertEqual(json.loads(log.read_text()),['-n','/usr/bin/cerynthctl',*args])
            log.unlink()

    def test_full_matrix_statistics(self):
        results = []
        for workload in experiment.WORKLOADS:
            for profile in experiment.PROFILES:
                for value in [1, 2, 3]:
                    results.append(dict(workload=workload, profile=profile, mean_cpu_usage_percent=value,
                                        mean_context_switch_rate=value, mean_process_creation_rate=value, elapsed_seconds=value))
        experiment.aggregate(self.root, results, 3)
        self.assertEqual(len(list((self.root / 'summaries').glob('*.json'))), 30)
        summary = json.loads((self.root / 'summaries/balanced-idle.json').read_text())
        self.assertEqual(summary['mean_cpu_usage_percent'], dict(mean=2, stdev=1, min=1, max=3))
        self.assertIn('±', (self.root / 'report.md').read_text())


if __name__ == '__main__':
    unittest.main()
