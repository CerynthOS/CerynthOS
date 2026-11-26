#!/usr/bin/env python3
"""Credential/consent preflight regressions; these do not run a cloud request."""
import unittest
import subprocess
import tempfile
from pathlib import Path
from cloud_routing_smoke import consent_grant, validate_config
from guard_runtime_smoke import stop_process
from inference_runtime_smoke import rpc
from resource_hierarchy_smoke import wait


class CloudPreflight(unittest.TestCase):
    @unittest.skipUnless(Path('target/debug/cerynth-inferenced').is_file(), 'build inferenced for the real policy integration check')
    def test_real_coordinator_checks_both_consent_boundaries_without_a_provider_request(self):
        with tempfile.TemporaryDirectory(prefix='cerynth-cloud-policy-') as temporary:
            root = Path(temporary)
            directory = root / 'inference'
            process = subprocess.Popen([str(Path('target/debug/cerynth-inferenced').resolve()), '--directory', str(directory), 'serve'],
                env={'PATH': '/usr/local/bin:/usr/bin:/bin', 'HOME': str(root), 'CERYNTH_TEST_KEY': 'public-fixture-not-a-provider-token'},
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                wait(lambda: (directory / 'control.sock').exists())
                credential = consent_grant(directory, 'https://provider.invalid/v1', 'fixture-model', 'CERYNTH_TEST_KEY')
                self.assertEqual(rpc(directory, True, {'type': 'metrics'})['samples'], [])
                self.assertEqual(rpc(directory, True, {'type': 'health'})['active_requests'], 0)
                rpc(directory, True, {'type': 'revoke', 'session_id': credential['session_id']})
                self.assertEqual(rpc(directory, True, {'type': 'health'})['grants'], 0)
            finally:
                stop_process(process)

    def test_explicit_consent_and_dedicated_variable_are_required(self):
        with self.assertRaises(ValueError):
            validate_config('https://provider.example/v1', 'fixture-model', 'CERYNTH_TEST_KEY', False)
        with self.assertRaises(ValueError):
            validate_config('https://provider.example/v1', 'fixture-model', 'OPENAI_API_KEY', True)
        self.assertEqual(validate_config('https://provider.example/v1', 'fixture-model', 'CERYNTH_TEST_KEY', True), 'provider.example')

    def test_unsafe_endpoints_fail_before_credentials_or_network(self):
        for endpoint in ['http://provider.example/v1', 'https://user:secret@provider.example/v1',
                         'https://provider.example/v1?key=secret', 'https://provider.example/v1#secret',
                         'https:///missing-host', 'https://provider.example/\nheader']:
            with self.subTest(endpoint=endpoint), self.assertRaises(ValueError):
                validate_config(endpoint, 'fixture-model', 'CERYNTH_TEST_KEY', True)


if __name__ == '__main__':
    unittest.main()
