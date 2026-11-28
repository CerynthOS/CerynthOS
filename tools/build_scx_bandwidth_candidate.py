#!/usr/bin/env python3
"""Build a separate pinned upstream LAVD candidate without changing the SCX baseline."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
from image_reproducibility import digest, tree_manifest

ROOT = Path(__file__).resolve().parents[1]
PIN = '67be0435c454dc8984a11064dc8b9c6476c46b6d'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--offline', action='store_true')
    args = parser.parse_args()
    source = ROOT / 'third_party/scx'; destination = ROOT / 'artifacts/person1/lavd-source'
    revision = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    if revision != PIN or subprocess.check_output(['git', '-C', str(source), 'status', '--porcelain'], text=True).strip():
        raise RuntimeError('upstream SCX checkout must be clean and match the frozen pin')
    destination.mkdir(parents=True, exist_ok=True)
    # Reduce only the copied workspace's member list. Otherwise Cargo resolves
    # every unrelated scheduler/UI dependency before building the one experiment.
    # Copy symlinks as symlinks, including their in-repository targets.
    for relative in ['rust', 'lib', 'scheds/rust/scx_lavd', 'scheds/include']:
        if (destination / relative).exists():
            shutil.rmtree(destination / relative)
        shutil.copytree(source / relative, destination / relative, dirs_exist_ok=True,
                        symlinks=True,
                        ignore=shutil.ignore_patterns('target', '__pycache__'))
    for name in ['Cargo.toml', 'Cargo.lock', 'LICENSE']:
        shutil.copyfile(source / name, destination / name)
    shutil.copyfile(ROOT / 'docs/contracts/scx-bandwidth-candidate.lock', destination / 'Cargo.lock')
    manifest = destination / 'Cargo.toml'; text = manifest.read_text()
    start = text.index('members = ['); end = text.index(']', start)
    manifest.write_text(text[:start] + 'members = ["scheds/rust/scx_lavd"]' + text[end + 1:])
    command = ['cargo', 'build', '--locked', '--manifest-path', str(manifest), '--profile', 'release-fast',
               '--target-dir', str(ROOT / 'target'), '-p', 'scx_lavd'] + (['--offline'] if args.offline else [])
    subprocess.run(command, check=True, cwd=ROOT, env=os.environ | {'CARGO_INCREMENTAL': '0', 'CARGO_BUILD_JOBS': '2'})
    # Verify that the executable sources were copied unchanged; resolution gets
    # its own retained lock and must not be called the main workspace lock.
    source_files = tree_manifest(source / 'scheds/rust/scx_lavd')
    copied_files = tree_manifest(destination / 'scheds/rust/scx_lavd')
    if source_files != copied_files:
        raise RuntimeError('candidate scheduler source changed during preparation')
    binary = ROOT / 'target/release-fast/scx_lavd'
    report = {'schema_version': 1, 'kind': 'pinned-upstream-bandwidth-candidate-build',
              'upstream_revision': revision, 'upstream_source_unchanged': True,
              'copied_workspace_members_reduced': True, 'profile': 'release-fast',
              'resolved_lock_sha256': digest(destination / 'Cargo.lock'), 'binary_sha256': digest(binary),
              'scheduler_source_sha256': hashlib.sha256(json.dumps(source_files, sort_keys=True).encode()).hexdigest(),
              'production_baseline_modified': False, 'rustc_version': subprocess.check_output(['rustc', '--version'], text=True).strip()}
    (destination / 'build.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
