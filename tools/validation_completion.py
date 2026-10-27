#!/usr/bin/env python3
"""Validate experiment completion independently of systemd's success-signal rules."""
import json
from pathlib import Path
import sys


def verify(phase, directory):
    root=Path(directory)
    if (root/'failure.json').exists():
        raise RuntimeError(f'{phase}: failure evidence exists')
    if phase in ['smoke','stability','diagnostic']:
        result=json.loads((root/'result.json').read_text())
        if result.get('status')!='pass' or result.get('watchdog_events')!=[] or result.get('completed_phases',0)<1:
            raise RuntimeError(f'{phase}: incomplete stability result')
    else:
        result=json.loads((root/'completion.json').read_text())
        expected=5 if phase=='adaptive' else 90 if phase=='matrix' else None
        if expected is None or result.get('status')!='pass' or result.get('mode')!=phase or result.get('completed_runs')!=expected:
            raise RuntimeError(f'{phase}: missing full completion evidence')
    return result


if __name__=='__main__':
    verify(sys.argv[1],sys.argv[2])
    print('experiment completion verified:',sys.argv[1])
