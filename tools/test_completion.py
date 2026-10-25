import os
import signal
import subprocess
import sys
import time
import json
from pathlib import Path
import tempfile
import unittest

from validation_completion import verify
from stability_source import quarantined_runs

ROOT=Path(__file__).resolve().parents[1]


class CompletionTests(unittest.TestCase):
    def test_service_success_without_complete_experiment_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            with self.assertRaises(OSError):
                verify('adaptive',root)
            (root/'completion.json').write_text(json.dumps(dict(status='pass',completed_runs=2,mode='adaptive')))
            with self.assertRaises(RuntimeError):
                verify('adaptive',root)
            (root/'completion.json').write_text(json.dumps(dict(status='pass',completed_runs=5,mode='adaptive')))
            verify('adaptive',root)
            (root/'failure.json').write_text('{"error":"interrupted"}')
            with self.assertRaises(RuntimeError):
                verify('adaptive',root)

    def test_quarantine_follows_binary_identity_not_retry_success(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            failed=root/'cerynth-validation-first'
            retry=root/'cerynth-validation-retry'
            for path in [failed,retry]:
                path.mkdir()
                (path/'source-revision.json').write_text(json.dumps(dict(binaries={'target/release/cerynth-scx':'bad-binary'})))
                (path/'stability').mkdir()
            (failed/'stability/failure.json').write_text('{"error":"watchdog"}')
            (retry/'stability/result.json').write_text('{"status":"pass"}')
            self.assertEqual(quarantined_runs(root,'bad-binary'),[str(failed)])
            self.assertEqual(quarantined_runs(root,'new-binary'),[])

    def test_matrix_failure_also_quarantines_the_binary(self):
        with tempfile.TemporaryDirectory() as temp:
            failed=Path(temp)/'cerynth-validation-matrix-failure'
            (failed/'benchmark-matrix').mkdir(parents=True)
            (failed/'source-revision.json').write_text(json.dumps(dict(binaries={'target/release/cerynth-scx':'startup-failure'})))
            (failed/'benchmark-matrix/failure.json').write_text('{"error":"watchdog on cold start"}')
            self.assertEqual(quarantined_runs(temp,'startup-failure'),[str(failed)])

    @unittest.skipUnless(sys.platform.startswith('linux'),'Linux fallback-only interruption test')
    def test_interrupted_experiment_exits_failed_and_cleans_workload(self):
        state=Path('/sys/kernel/sched_ext/state')
        if not state.exists() or state.read_text().strip()!='disabled':
            self.skipTest('requires Linux fallback; never change the running scheduler')
        with tempfile.TemporaryDirectory() as temp:
            output=Path(temp)/'experiment'
            process=subprocess.Popen([sys.executable,str(ROOT/'benchmarks/experiment.py'),'adaptive','--output',str(output)],
                                     stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
            try:
                deadline=time.monotonic()+8
                while time.monotonic()<deadline:
                    if any(p.stat().st_size for p in output.glob('raw/*/observations.jsonl')):
                        break
                    if process.poll() is not None:
                        self.fail('experiment exited before interruption: '+process.communicate()[1])
                    time.sleep(.05)
                else:
                    self.fail('idle workload did not start')
                process.send_signal(signal.SIGTERM)
                stdout,stderr=process.communicate(timeout=8)
                self.assertEqual(process.returncode,1,stderr)
                self.assertIn('interrupted by signal',json.loads((output/'failure.json').read_text())['error'])
                self.assertFalse((output/'completion.json').exists())
                with self.assertRaises(RuntimeError):
                    verify('adaptive',output)
                self.assertEqual(state.read_text().strip(),'disabled')
            finally:
                if process.poll() is None:
                    process.send_signal(signal.SIGTERM)
                    process.communicate(timeout=8)


if __name__=='__main__':
    unittest.main()
