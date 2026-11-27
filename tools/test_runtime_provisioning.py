"""Provisioning must preserve unknown files and keep missing pins a failed gate."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from provision_pinned_cli import provision, ASSETS

ROOT=Path(__file__).resolve().parents[1]


class RuntimeProvisioningTests(unittest.TestCase):
    def test_unknown_executable_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);target=root/'codex-0.167.0';target.write_bytes(b'existing executable')
            with patch('urllib.request.urlopen') as network:
                with self.assertRaises(ValueError):provision('codex',root)
                network.assert_not_called()
            self.assertEqual(target.read_bytes(),b'existing executable')

    def test_corrupt_cached_archive_is_rejected_without_network(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);(root/Path(ASSETS['codex']['url']).name).write_bytes(b'wrong bytes')
            with patch('urllib.request.urlopen') as network:
                with self.assertRaises(ValueError):provision('codex',root)
                network.assert_not_called()
            self.assertFalse((root/'codex-0.167.0').exists())

    def test_missing_pins_emit_report_and_fail_even_with_python_optimization(self):
        with tempfile.TemporaryDirectory() as temporary:
            report=Path(temporary)/'inventory.json'
            environment=os.environ.copy()
            for name in ['CODEX','CLAUDE','OPENCODE','HERMES','OLLAMA']:
                environment['CERYNTH_'+name+'_BINARY']='/nonexistent/cerynth-pin-test'
            result=subprocess.run([sys.executable,'-O',str(ROOT/'tools/runtime_version_inventory.py'),'--output',str(report)],
                env=environment,capture_output=True,text=True,timeout=60)
            self.assertEqual(result.returncode,1)
            self.assertNotIn('Traceback',result.stderr)
            data=json.loads(report.read_text())
            self.assertFalse(data['all_local_pins_match'])
            self.assertTrue(all(e['error']=='pinned_executable_missing' and not e['pin_matches'] for e in data['executables']))
            self.assertEqual(data['cloud_requests'],0)


if __name__=='__main__':unittest.main()
