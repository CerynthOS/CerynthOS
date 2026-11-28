#!/usr/bin/env python3
"""Run local release checks and fail qualification when external acceptance is missing.

No cloud credentials are discovered or used. This command never creates human
approvals and does not equate successful local tests with a qualified release.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = ['cerynth-agent-api', 'cerynth-agent-protocols', 'cerynth-model-api', 'cerynth-resources',
            'cerynth-adapter-cli-harnesses', 'cerynth-adapter-generic-pty', 'cerynth-agentd', 'cerynth-inferenced', 'cerynthd']


def outstanding_tasks():
    content = (ROOT / 'docs/roadmap/person3-tasks.md').read_text()
    return [{'task': task, 'status': status, 'acceptance': reason.strip()} for task, status, reason in
            re.findall(r'^\| (P3-\d+) \|[^\n]*?\| (Partial|Pending) \| (.*?) \|$', content, re.M)]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', default='artifacts/person3/release-qualification.json')
    parser.add_argument('--run-local', action='store_true')
    parser.add_argument('--pinned-cli-directory', type=Path, help='explicitly select the pinned workspace CLI installations for every local check')
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    logs = output.parent / (output.stem + '-logs')
    logs.mkdir(mode=0o700, exist_ok=True)
    cargo = os.environ.get('CARGO', str(Path.home() / '.cargo/bin/cargo'))
    environment = {**os.environ, 'CARGO_INCREMENTAL': '0', 'CARGO': cargo}
    if args.pinned_cli_directory:
        tools = args.pinned_cli_directory.resolve()
        for variable, suffix in {'CERYNTH_CODEX_BINARY':'codex-0.167.0', 'CERYNTH_CLAUDE_BINARY':'claude-2.1.312',
                'CERYNTH_OPENCODE_BINARY':'opencode-1.19.5', 'CERYNTH_HERMES_BINARY':'hermes-0.23.1/.venv/bin/hermes'}.items():
            environment[variable] = str(tools / suffix)
    commands = [
        ('format', [cargo, 'fmt', '--all', '--', '--check']),
        ('workspace-check', [cargo, 'check', '--locked', '--workspace', '--all-targets']),
        ('workspace-tests', [cargo, 'test', '--locked', '--workspace']),
        ('strict-lint', [cargo, 'clippy', '--locked', '--no-deps', *sum((['-p', name] for name in PACKAGES), []), '--all-targets', '--', '-D', 'warnings']),
        ('contracts', ['bash', 'scripts/check-agent-contracts.sh']),
        ('shellcheck', ['shellcheck', '-S', 'warning', 'scripts/check-agent-contracts.sh', 'scripts/install-agent-runtime.sh']),
        ('version-pins', ['python3', 'tools/runtime_version_inventory.py', '--output', str(logs / 'version-pins.json')]),
        ('cloud-consent-policy-tests', ['python3', '-m', 'unittest', 'discover', '-s', 'tools', '-p', 'test_cloud_routing_smoke.py', '-v']),
    ]
    checks = []
    if args.run_local:
        for name, command in commands:
            started = time.monotonic()
            destination = logs / (name + '.log')
            with destination.open('wb') as stream:
                try:
                    code = subprocess.run(command, cwd=ROOT, env=environment, stdout=stream, stderr=subprocess.STDOUT, timeout=900).returncode
                except subprocess.TimeoutExpired:
                    code = 124
                except OSError:
                    code = 127
            checks.append({'name': name, 'passed': code == 0, 'return_code': code,
                           'elapsed_seconds': time.monotonic() - started,
                           'log_sha256': hashlib.sha256(destination.read_bytes()).hexdigest()})
    missing = outstanding_tasks()
    report = {'kind': 'person3-release-qualification',
              'runner_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'explicit_harness_selection_names': sorted(k for k in environment if k in ['CERYNTH_CODEX_BINARY', 'CERYNTH_CLAUDE_BINARY', 'CERYNTH_OPENCODE_BINARY', 'CERYNTH_HERMES_BINARY']),
              'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'tracked_worktree_clean': not subprocess.check_output(['git', 'diff', 'HEAD', '--'], cwd=ROOT),
              'local_checks_run': args.run_local, 'local_checks': checks,
              'local_checks_passed': bool(checks) and all(c['passed'] for c in checks),
              'remaining_acceptance': missing, 'team_approval_claimed': False,
              'cloud_requests': 0, 'release_qualified': False}
    report['automated_gate_passed'] = report['local_checks_passed'] and not missing
    # A local audit cannot supply or verify human approvals or live provider evidence.
    # The ledger must be backed by those actual results before a separate release review.
    output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    return 0 if report['automated_gate_passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
