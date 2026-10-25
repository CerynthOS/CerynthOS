"""Run the production C callback with kernel-contract stubs; no BPF privileges."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCHEDULER = ROOT/'schedulers/cerynth-scx'


class DispatchProgressTests(unittest.TestCase):
    def build_and_run(self, header=None):
        compiler = shutil.which('cc')
        self.assertIsNotNone(compiler, 'C compiler required for the callback regression')
        with tempfile.TemporaryDirectory() as temp:
            include = Path(temp)
            if header is not None:
                (include/'dispatch.h').write_text(header)
            binary = include/'drain-test'
            subprocess.run([compiler, '-std=c11', '-Wall', '-Wextra', '-Werror', '-I', str(include),
                            '-I', str(SCHEDULER), str(SCHEDULER/'tests/dispatch-progress.c'), '-o', str(binary)],check=True,capture_output=True)
            return subprocess.run([str(binary)],capture_output=True,text=True)

    def test_production_callback_preserves_forward_progress(self):
        result = self.build_and_run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('exact-once delivery passed', result.stdout)

    def test_regression_harness_rejects_previous_inverted_condition(self):
        old = (SCHEDULER/'dispatch.h').read_text().replace('scx_bpf_dispatch_nr_slots() == 0', '!!scx_bpf_dispatch_nr_slots()')
        result = self.build_and_run(old)
        self.assertNotEqual(result.returncode, 0, 'the old callback must reproduce the drain regression')


if __name__ == '__main__':
    unittest.main()
