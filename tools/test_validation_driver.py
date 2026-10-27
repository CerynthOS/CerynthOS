"""Unprivileged driver tests: all system commands are replaced by local stubs."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from validation_manifest import manifest

TOOLS = Path(__file__).resolve().parent


class DriverTests(unittest.TestCase):
    def test_cleanup_order_and_failure_recovery(self):
        for code, restore_failure, stop_failure, changed, diagnostic, recovery in [
            (0, 0, 0, 0, 0, 'updated-daemon-balanced'),
            (7, 0, 0, 0, 0, 'previous-files-linux-default'),
            (9, 1, 0, 0, 0, 'linux-default-restore-failed'),
            (0, 0, 1, 0, 0, 'previous-files-linux-default'),
            (0, 0, 0, 1, 0, 'previous-files-linux-default'),
            (0, 0, 0, 0, 1, 'previous-files-linux-default'),
        ]:
            with self.subTest(code=code, restore_failure=restore_failure, stop_failure=stop_failure), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                prior = root/'prior'
                prior.mkdir()
                (prior/'evidence.json').write_text('{"preserved":true}')
                (root/'baseline-before.json').write_text(json.dumps(manifest(prior), indent=2, sort_keys=True)+'\n')
                if changed:
                    (prior/'evidence.json').write_text('{"changed":true}')
                harness = r'''
source "$1"
SOURCE="$2"
RESULT="$3"
OLD_MATRIX="$4"
RUN_USER=fixture
RULE="$RESULT/temporary-rule"
RULE_TEMP=''
RULE_CREATED=1
MUTATED=1
LAUNCHER_PID=''
UNITS=()
FAKE_STATE=enabled
printf 'fixture\n' > "$RULE"
record() { printf '%s\n' "$1" >> "$RESULT/order.log"; }
stop_validation_units() { record stop-workloads; return "$STOP_FAILURE"; }
restore_previous_runtime() { record restore-runtime; return "$RESTORE_FAILURE"; }
healthy() { record verify-health; return 0; }
capture_failure() { record capture-evidence; }
linux_fallback() { record linux-fallback; FAKE_STATE=disabled; return 0; }
ctl() { record final-status; printf 'Balanced\n'; }
systemctl() { printf 'active\n'; }
runuser() { shift 3; "$@"; }
cat() {
  if [[ "$1" == /sys/kernel/sched_ext/state ]]; then printf '%s\n' "$FAKE_STATE";
  else command cat "$@"; fi
}
rm() { record remove-rule; command rm "$@"; }
visudo() { record validate-sudoers; printf 'parsed OK\n'; }
chown() { :; }
trap cleanup EXIT
exit "$INJECTED_CODE"
'''
                env = {**os.environ, 'INJECTED_CODE': str(code), 'RESTORE_FAILURE': str(restore_failure), 'STOP_FAILURE': str(stop_failure), 'DIAGNOSTIC_ONLY': str(diagnostic)}
                result = subprocess.run(['bash','-c',harness,'harness',str(TOOLS/'run-vm-validation.sh'),str(TOOLS.parent),str(root),str(prior)],
                                        env=env,capture_output=True,text=True)
                expected = 1 if restore_failure or stop_failure or changed else code
                self.assertEqual(result.returncode, expected, result.stderr)
                order = (root/'order.log').read_text().splitlines()
                self.assertIn('stop-workloads', order)
                recovery_step = 'restore-runtime' if code or stop_failure or changed or diagnostic else 'verify-health'
                self.assertLess(order.index('stop-workloads'), order.index(recovery_step))
                self.assertLess(order.index(recovery_step), order.index('remove-rule'))
                if code or stop_failure or changed or diagnostic:
                    self.assertLess(order.index('capture-evidence'), order.index('linux-fallback'))
                    self.assertLess(order.index('linux-fallback'), order.index('restore-runtime'))
                self.assertLess(order.index('remove-rule'), order.index('validate-sudoers'))
                self.assertFalse((root/'temporary-rule').exists())
                report = json.loads((root/'cleanup-result.json').read_text())
                self.assertEqual(report['recovery'], recovery)
                self.assertEqual(report['temporary_rule'], 'absent')
                self.assertEqual(report['baseline'], 'changed' if changed else 'unchanged')
                self.assertEqual(report['workloads'], 'failed' if stop_failure else 'ok')

    def test_allocator_setting_restoration_is_limited_to_saved_boolean_values(self):
        with tempfile.TemporaryDirectory() as temp:
            setting=Path(temp)/'allocator-setting'
            setting.write_text('0\n')
            command='source "$1"; restore_allocator_setting "$2" "$3"'
            result=subprocess.run(['bash','-c',command,'fixture',str(TOOLS/'run-vm-validation.sh'),str(setting),'1'],capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(setting.read_text(),'1\n')
            for invalid in ['2','', '../unexpected']:
                result=subprocess.run(['bash','-c',command,'fixture',str(TOOLS/'run-vm-validation.sh'),str(setting),invalid],capture_output=True,text=True)
                self.assertNotEqual(result.returncode,0)
                self.assertEqual(setting.read_text(),'1\n')

    def test_sudo_rule_contains_only_exact_argument_lists(self):
        result = subprocess.run(['bash','-c','source "$1"; RUN_USER=fixture; sudo_rule','harness',str(TOOLS/'run-vm-validation.sh')],capture_output=True,text=True,check=True)
        entries = result.stdout.strip().split('NOPASSWD: ',1)[1].split(', ')
        self.assertEqual(entries, ['/usr/bin/cerynthctl '+args for args in [
            'status','start','stop','profile set balanced','profile set interactive','profile set performance','profile set background']])
        self.assertFalse(any(char in result.stdout for char in '*?'))

    def test_configuration_rejects_missing_or_unsafe_inputs(self):
        for args in [[], ['--source'], ['--unknown'],
                     ['--source', '.', '--run-user', 'root', '--baseline', '.'],
                     ['--source', '/tmp', '--run-user', 'root', '--baseline', '/tmp'],
                     ['--source', '/tmp', '--run-user', 'bad,account', '--baseline', '/tmp']]:
            result = subprocess.run(['bash', '-c', 'source "$1"; shift; configure_validation "$@"',
                                     'fixture', str(TOOLS/'run-vm-validation.sh'), *args], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0, args)

    def test_configuration_accepts_explicit_nonroot_account(self):
        import pwd
        if os.getuid() == 0:
            self.skipTest('requires a normal user account')
        with tempfile.TemporaryDirectory() as temp:
            result = subprocess.run(['bash', '-c',
                'source "$1"; shift; configure_validation "$@"; printf "%s\\n" "$SOURCE" "$OLD_MATRIX" "$RUN_USER" "$DIAGNOSTIC_ONLY"',
                'fixture', str(TOOLS/'run-vm-validation.sh'), '--source', temp,
                '--baseline', temp, '--run-user', pwd.getpwuid(os.getuid()).pw_name,
                '--diagnostic-only'], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.splitlines(), [temp, temp, pwd.getpwuid(os.getuid()).pw_name, '1'])

    def test_manifest_rejects_symlinks(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root/'file').write_text('evidence')
            (root/'link').symlink_to(root/'file')
            with self.assertRaises(ValueError):
                manifest(root)


if __name__ == '__main__':
    unittest.main()
