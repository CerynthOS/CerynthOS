#!/usr/bin/env python3
"""One explicitly consented compatible cloud request through real inferenced.

Uses a dedicated variable only. Configuration validation and policy-denial checks
are not a live cloud success; only a verified provider response qualifies.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
from urllib.parse import urlsplit
import uuid

from guard_runtime_smoke import stop_process
from inference_runtime_smoke import rpc
from resource_hierarchy_smoke import wait


def validate_config(endpoint, model, credential_env, consent):
    if not consent:
        raise ValueError('explicit cloud consent is required')
    parsed = urlsplit(endpoint)
    if (parsed.scheme != 'https' or not parsed.hostname or parsed.username is not None
            or parsed.password is not None or parsed.query or parsed.fragment
            or any(c.isspace() or ord(c) < 32 for c in endpoint)):
        raise ValueError('cloud endpoint must be HTTPS without credentials, query or fragment')
    if not model.strip() or len(model) > 256 or any(ord(c) < 32 for c in model):
        raise ValueError('a supported model identifier is required')
    if not re.fullmatch(r'CERYNTH_[A-Z0-9_]{1,119}', credential_env):
        raise ValueError('select a dedicated CERYNTH_ credential variable, not an ambient provider credential')
    return parsed.hostname


def consent_grant(directory, endpoint_url, model, credential_env):
    endpoint = {'kind': 'compatible', 'id': 'dedicated-cloud', 'config': {
        'url': endpoint_url, 'cloud_consent': False, 'credential_env': credential_env}}
    try:
        rpc(directory, True, {'type': 'configure', 'endpoint': endpoint})
        raise AssertionError('nonconsensual remote route was configured')
    except RuntimeError as error:
        if 'explicit consent and HTTPS' not in str(error):
            raise
    endpoint['config']['cloud_consent'] = True
    rpc(directory, True, {'type': 'configure', 'endpoint': endpoint})
    session = str(uuid.uuid4())
    grant = {'session_id': session, 'endpoint_id': 'dedicated-cloud',
             'trust_domain': 'public-cloud-smoke', 'allowed_models': [model],
             'concurrency_limits': [[session, 1]], 'max_priority': 0, 'allow_cloud': False}
    try:
        rpc(directory, True, {'type': 'register', 'grant': grant})
        raise AssertionError('session without cloud consent was registered')
    except RuntimeError as error:
        if 'session has not consented to remote inference' not in str(error):
            raise
    grant['allow_cloud'] = True
    credential = rpc(directory, True, {'type': 'register', 'grant': grant})
    return credential


def run(args):
    # Validate public configuration before looking up any credential.
    host = validate_config(args.endpoint, args.model, args.credential_env, args.consent)
    if not os.environ.get(args.credential_env):
        raise ValueError('the selected dedicated credential variable is not configured')
    binary = Path(args.binary).resolve()
    if not binary.is_file():
        raise ValueError('build the inference coordinator first')
    report = {'kind': 'actual-consented-compatible-cloud-routing', 'endpoint_host': host,
              'model': args.model, 'explicit_consent': True, 'cloud_requests_attempted': 0,
              'live_response_verified': False, 'max_output_tokens': args.max_output_tokens}
    process = None
    temporary = tempfile.TemporaryDirectory(prefix='cerynth-cloud-smoke-')
    try:
        root = Path(temporary.name)
        directory = root / 'inference'
        environment = {'PATH': '/usr/local/bin:/usr/bin:/bin', 'HOME': str(root),
                       args.credential_env: os.environ[args.credential_env]}
        process = subprocess.Popen([str(binary), '--directory', str(directory), 'serve'],
            env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

        def ready():
            if process.poll() is not None:
                raise RuntimeError('coordinator exited before readiness')
            return (directory / 'control.sock').exists()
        wait(ready)
        credential = consent_grant(directory, args.endpoint, args.model, args.credential_env)
        report['endpoint_consent_enforced'] = report['session_consent_enforced'] = True
        session = credential['session_id']
        challenge = 'CERYNTH_' + uuid.uuid4().hex[:8]
        request = {'session_id': session, 'token': credential['token'], 'model': args.model,
                   'prompt': 'Reply with exactly this public test string and nothing else: ' + challenge,
                   'max_output_tokens': args.max_output_tokens, 'priority': 0}
        report['cloud_requests_attempted'] = 1
        started = time.monotonic()
        response = rpc(directory, False, request)
        report['request_wall_ms'] = (time.monotonic() - started) * 1000
        report['live_response_verified'] = response['response']['message']['content'].strip() == challenge
        report['metrics'] = response['metrics']
        samples = rpc(directory, True, {'type': 'metrics'})['samples']
        report['session_usage_attributed'] = bool(samples) and all(s['session_id'] == session for s in samples)
        rpc(directory, True, {'type': 'revoke', 'session_id': session})
        # A rejected replay is checked after revocation; it cannot reach the provider.
        try:
            rpc(directory, False, request)
            raise AssertionError('revoked session reached inference')
        except RuntimeError as error:
            if 'unknown session grant' not in str(error):
                raise
        report['revoked_credential_rejected'] = True
        report['all_grants_cleaned'] = rpc(directory, True, {'type': 'health'})['grants'] == 0
        if not (report['live_response_verified'] and report['session_usage_attributed'] and report['all_grants_cleaned']):
            raise RuntimeError('provider response did not satisfy the independent public challenge')
    except Exception as error:
        # Provider bodies, command output, tokens and raw exceptions are private.
        report['failure_class'] = type(error).__name__
        if match := re.search(r'backend returned HTTP (\d{3})', str(error)):
            report['backend_http_status'] = int(match.group(1))
        raise
    finally:
        if process is not None:
            stop_process(process)
        report['coordinator_stopped'] = process is None or process.poll() is not None
        temporary.cleanup()
        destination = Path(args.output)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(json.dumps(report, indent=2) + '\n')
    return report


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--endpoint', required=True)
    parser.add_argument('--model', required=True)
    parser.add_argument('--credential-env', required=True)
    parser.add_argument('--consent', action='store_true')
    parser.add_argument('--max-output-tokens', type=int, choices=range(1, 65), default=32, metavar='1..64')
    parser.add_argument('--binary', default='target/debug/cerynth-inferenced')
    parser.add_argument('--output', default='artifacts/person3/cloud-routing-smoke.json')
    args = parser.parse_args()
    try:
        report = run(args)
    except Exception:
        parser.exit(1, 'Cloud validation did not pass; check configuration and any sanitized report.\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
