#!/usr/bin/env python3
"""Capture/verify source and binary identities for a stability experiment."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess

ROOT=Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_files():
    paths={ROOT/'Cargo.toml',ROOT/'Cargo.lock'}
    for name in ['schedulers/cerynth-scx','services/cerynthd','crates','cli','tools','tests/workloads','benchmarks']:
        paths.update(path for path in (ROOT/name).rglob('*') if path.is_file() and '__pycache__' not in path.parts
                     and (path.suffix in ['.rs','.c','.h','.py','.sh','.toml','.lock'] or path.name in ['validation-ctl','cerynth-observe']))
    return {str(path.relative_to(ROOT)):digest(path) for path in sorted(paths)}


def capture(base):
    dependency=ROOT/'third_party/scx'
    revision=subprocess.check_output(['git','-C',str(dependency),'rev-parse','HEAD'],text=True).strip()
    dep_files={}
    for directory in ['rust/scx_rustland_core','rust/scx_cargo','rust/scx_utils','scheds/include']:
        for path in (dependency/directory).rglob('*'):
            if path.is_file() and path.suffix in ['.rs','.h','.c','.toml']:
                dep_files[str(path.relative_to(ROOT))]=digest(path)
    record=dict(base_revision=base,dependency_revision=revision,source_files=source_files(),dependency_files=dep_files,
                kernel=platform.release(),kernel_btf_sha256=digest(Path('/sys/kernel/btf/vmlinux')),
                toolchain={tool:subprocess.check_output([tool,'--version'],text=True).splitlines()[0] for tool in ['rustc','cargo','clang']},binaries={f'target/release/{name}':digest(ROOT/f'target/release/{name}')
                                                   for name in ['cerynthd','cerynth-scx']})
    identity={key:record[key] for key in ['base_revision','dependency_revision','source_files','dependency_files']}
    record['entrypoint_sha256']=digest(ROOT/'run-privileged-validation.sh')
    record['source_version']=hashlib.sha256(json.dumps(identity,sort_keys=True).encode()).hexdigest()
    return record


def quarantined_runs(artifacts, binary_hash):
    failed=[]
    for directory in Path(artifacts).glob('cerynth-validation-*'):
        manifest=directory/'source-revision.json'
        if not manifest.exists():
            continue
        record=json.loads(manifest.read_text())
        if record.get('binaries',{}).get('target/release/cerynth-scx') != binary_hash:
            continue
        if any((directory/name/'failure.json').exists() for name in ['smoke','stability','diagnostic','adaptive-evaluation','benchmark-matrix']):
            failed.append(str(directory))
    return sorted(failed)


def verify(record):
    assert record['entrypoint_sha256']==digest(ROOT/'run-privileged-validation.sh'), 'entrypoint changed'
    assert record['kernel']==platform.release(), 'kernel changed since build'
    assert record['kernel_btf_sha256']==digest(Path('/sys/kernel/btf/vmlinux')), 'kernel BTF changed'
    for section in ['source_files','dependency_files','binaries']:
        for relative,expected in record[section].items():
            assert digest(ROOT/relative)==expected, f'changed {section}: {relative}'
    print('verified source version:',record['source_version'])


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode',choices=['capture','verify','quarantine'])
    parser.add_argument('path',type=Path)
    parser.add_argument('--base-revision')
    args=parser.parse_args()
    if args.mode=='capture':
        if not args.base_revision:
            parser.error('--base-revision required for capture')
        args.path.write_text(json.dumps(capture(args.base_revision),indent=2,sort_keys=True)+'\n')
    record=json.loads(args.path.read_text())
    verify(record)
    if args.mode=='quarantine':
        failures=quarantined_runs(ROOT/'artifacts',record['binaries']['target/release/cerynth-scx'])
        if failures:
            parser.error('scheduler binary failed targeted stability; full validation blocked; diagnostic-only is available: '+', '.join(failures))
