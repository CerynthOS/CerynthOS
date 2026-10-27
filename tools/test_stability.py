import importlib.util
import hashlib
from unittest.mock import patch
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('stability',ROOT/'tools/cerynth-stability.py')
stability=importlib.util.module_from_spec(spec)
spec.loader.exec_module(stability)


class StabilityTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name)
        (self.root/'kernel/sched_ext').mkdir(parents=True)
        (self.root/'kernel/sched_ext/state').write_text('enabled')
        (self.root/'70').mkdir()
        self.process_state('S')
        self.status=dict(pid=70,running=True,healthy=True,profile='Interactive',heartbeat=time.time())
        self.stats=dict(pid=70,heartbeat=time.time(),progress_sequence=100,dispatch_errors=0,dequeue_errors=0)
        self.write()

    def process_state(self,state):
        fields=[state,'1']+['0']*20
        fields[11]='10'
        fields[12]='20'
        (self.root/'70/stat').write_text('70 (cerynth-scx) '+' '.join(fields))

    def write(self):
        (self.root/'scheduler.json').write_text(json.dumps(self.status))
        (self.root/'scheduler-stats.json').write_text(json.dumps(self.stats))

    def health(self,profile='interactive'):
        return stability.check_health(profile,self.root,self.root,self.root)

    def test_live_profile_and_progress(self):
        self.assertEqual(self.health()['process_cpu_ticks'],30)
        for profile in ['Balanced','Interactive','Performance','Background','Interactive']:
            self.status['profile']=profile
            self.write()
            self.assertEqual(self.health(profile.lower())['scheduler']['profile'],profile)

    def test_stale_heartbeat_or_prior_process_is_rejected(self):
        self.stats['pid']=69
        self.write()
        with self.assertRaisesRegex(RuntimeError,'another process'):
            self.health()
        self.stats['pid']=70
        self.status['heartbeat']=time.time()-10
        self.write()
        with self.assertRaisesRegex(RuntimeError,'heartbeat'):
            self.health()

    def test_dead_scheduler_is_not_alive_just_because_pid_exists(self):
        self.process_state('Z')
        with self.assertRaisesRegex(RuntimeError,'zombie'):
            self.health()
        (self.root/'70/stat').unlink()
        with self.assertRaises(OSError):
            self.health()

    def test_integer_heartbeat_quantization_does_not_invent_staleness(self):
        self.status['heartbeat']=1000
        self.stats['heartbeat']=1000
        self.write()
        with patch.object(stability.time,'time',return_value=1003.1):
            self.health()
        with patch.object(stability.time,'time',return_value=1004):
            with self.assertRaisesRegex(RuntimeError,'heartbeat'):
                self.health()
        with patch.object(stability.time,'time',return_value=999.9):
            with self.assertRaisesRegex(RuntimeError,'heartbeat'):
                self.health()

    def test_kernel_disablement_and_transition_are_failures(self):
        for state in ['disabled','disabling','enabling']:
            (self.root/'kernel/sched_ext/state').write_text(state)
            with self.assertRaisesRegex(RuntimeError,'sched_ext='):
                self.health()

    def test_watchdog_is_not_ignored_as_an_ordinary_stop(self):
        rows=[{'MESSAGE':'sched_ext: disabled (unregistered from user space)'},
              {'MESSAGE':'sched_ext: disabled (runnable task stall)'},
              {'MESSAGE':'sched_ext: watchdog failed to check in for 5s'}]
        self.assertEqual(stability.watchdog_rows(rows),rows[1:])

    def test_kernel_watchdog_latches_no_restart(self):
        events=stability.KernelEvents.__new__(stability.KernelEvents)
        events.cursor='before'
        events.events=[]
        events.watchdog_detected=False
        events.unsafe_to_restart=False
        events.query=lambda arguments:[{'__CURSOR':'after','MESSAGE':'sched_ext: disabled (runnable task stall)'}]
        with self.assertRaises(stability.WatchdogFailure):
            events.poll()
        self.assertTrue(events.watchdog_detected)
        self.assertTrue(events.unsafe_to_restart)
        self.assertEqual(len(events.events),1)

    def test_watchdog_stop_or_forced_kill_blocks_reattach(self):
        stability.check_clean_stop(dict(pid=70,running=False,healthy=False),70)
        for record in [dict(pid=70,running=False,healthy=False,exit_error='watchdog stall'),
                       dict(pid=70,running=True,healthy=True),
                       dict(pid=69,running=False,healthy=False)]:
            with self.assertRaisesRegex(RuntimeError,'refusing reattach'):
                stability.check_clean_stop(record,70)

    def test_matrix_gate_rejects_watchdog_failed_or_wrong_revision(self):
        spec=importlib.util.spec_from_file_location('matrix_gate',ROOT/'benchmarks/experiment.py')
        module=importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        binary=self.root/'scheduler'
        binary.write_bytes(b'candidate-source-version')
        digest=hashlib.sha256(binary.read_bytes()).hexdigest()
        parameters=dict(rounds=1,duration=5,idle_soak=5,workers=32,scheduler_sha256=digest,kernel=module.platform.release())
        smoke=self.root/'smoke.json'
        sustained=self.root/'sustained.json'
        smoke.write_text(json.dumps(dict(status='pass',completed_phases=6,watchdog_events=[],parameters=parameters)))
        long_params={**parameters,'rounds':3,'duration':15,'idle_soak':120,'linux_baseline':30}
        sustained.write_text(json.dumps(dict(status='pass',completed_phases=18,completed_linux_baselines=3,watchdog_events=[],parameters=long_params)))
        module.verify_stability_gates([smoke,sustained],binary)
        good=dict(status='pass',completed_phases=18,completed_linux_baselines=3,watchdog_events=[],parameters=long_params)
        for bad in [{**good,'completed_linux_baselines':0},
                    {**good,'parameters':{**long_params,'linux_baseline':0}},
                    {**good,'parameters':{**long_params,'idle_soak':60}},
                    {**good,'status':'fail'},
                    {**good,'watchdog_events':['watchdog']},
                    {**good,'parameters':{**long_params,'scheduler_sha256':'wrong'}}]:
            sustained.write_text(json.dumps(bad))
            with self.assertRaises(RuntimeError):
                module.verify_stability_gates([smoke,sustained],binary)

    def test_linux_baseline_completes_and_rejects_wrong_kernel_state(self):
        class Kernel:
            def poll(self): pass
        state=self.root/'kernel/sched_ext/state'
        state.write_text('disabled')
        output=self.root/'baseline'
        result=stability.run_linux_baseline(Kernel(),[sys.executable,'-c','print("baseline finished")'],output,.01,state)
        self.assertEqual(result['exit_code'],0)
        self.assertIn('baseline finished',(output/'workload.log').read_text())
        rows=[json.loads(line) for line in (output/'observations.jsonl').read_text().splitlines()]
        self.assertTrue(rows)
        self.assertTrue(all(row['sched_ext']=='disabled' for row in rows))
        state.write_text('enabled')
        with self.assertRaisesRegex(RuntimeError,'requires sched_ext disabled'):
            stability.run_linux_baseline(Kernel(),[sys.executable,'-c','raise AssertionError()'],self.root/'blocked',.01,state)
        self.assertFalse((self.root/'blocked').exists())

    def test_linux_baseline_failure_stops_before_cold_start(self):
        class Kernel:
            def poll(self): pass
        state=self.root/'kernel/sched_ext/state'
        state.write_text('disabled')
        with self.assertRaisesRegex(RuntimeError,'Linux baseline exited 7'):
            stability.run_linux_baseline(Kernel(),[sys.executable,'-c','import sys;sys.exit(7)'],self.root/'failed-baseline',.01,state)

    @unittest.skipUnless(sys.platform.startswith('linux'),'Linux fork/barrier workload')
    def test_all_burst_workers_finish(self):
        path=self.root/'progress.json'
        result=subprocess.run([sys.executable,str(ROOT/'tests/workloads/scheduler-burst.py'),
                               '--duration','.4','--workers','4','--cpu-ms','2','--output',str(path)],
                              capture_output=True,text=True,timeout=15)
        self.assertEqual(result.returncode,0,result.stderr)
        progress=json.loads(path.read_text())
        self.assertTrue(progress['completed'])
        self.assertGreater(progress['completed_bursts'],0)
        self.assertEqual(progress['worker_completions'],[progress['completed_bursts']]*4)


if __name__=='__main__':
    unittest.main()
