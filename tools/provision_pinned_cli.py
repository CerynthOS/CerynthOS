#!/usr/bin/env python3
"""Provision exact public CLI artifacts in the workspace; never replace host programs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
ASSETS = {
    'codex': {'version': '0.167.0', 'url': 'https://github.com/openai/codex/releases/download/rust-v0.167.0/codex-x86_64-unknown-linux-musl.tar.gz',
              'archive_sha256': '9d64836106d5b374e90ee4454c7ebcde7ca7106558bd7e41a34b9f26ebda251a',
              'binary_sha256': 'cdcb45aaa01d0f14e298985e09465aebccd47f64ecd298aea44814af7b62bbda',
              'member': 'codex-x86_64-unknown-linux-musl', 'environment': 'CERYNTH_CODEX_BINARY'},
    'opencode': {'version': '1.19.5', 'url': 'https://github.com/anomalyco/opencode/releases/download/v1.19.5/opencode-linux-x64.tar.gz',
                 'archive_sha256': '588310a336fa3b9df36b3f0225b26f679d453efaad0cf3d66c3651ef3ec1647a',
                 'binary_sha256': '7df4e4136301f16589b0f30bf91a61380fca5f8ef22dd36570f5ebe1d4ec802c',
                 'member': 'opencode', 'environment': 'CERYNTH_OPENCODE_BINARY'},
}


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def provision(name, directory):
    record = ASSETS[name]
    target = directory / (name + '-' + record['version'])
    if target.exists():
        if target.is_symlink() or not target.is_file() or sha(target) != record['binary_sha256']:
            raise ValueError('refusing to overwrite an unexpected existing executable: ' + str(target))
        return target
    archive = directory / Path(record['url']).name
    if not archive.exists():
        with tempfile.NamedTemporaryFile(dir=directory, delete=False) as stream:
            temporary = Path(stream.name)
            try:
                with urllib.request.urlopen(record['url'], timeout=45) as response:
                    shutil.copyfileobj(response, stream)
                stream.flush()
                if sha(temporary) != record['archive_sha256']:
                    raise ValueError('download differs from pinned release digest')
            except BaseException:
                temporary.unlink(missing_ok=True); raise
        temporary.replace(archive)
    if sha(archive) != record['archive_sha256']:
        raise ValueError('cached archive differs from pinned release digest')
    with tarfile.open(archive) as source:
        members = [m for m in source.getmembers() if m.isfile() and Path(m.name).name == record['member']]
        if len(members) != 1 or members[0].size > 512 * 1024**2:
            raise ValueError('unexpected release archive layout')
        with tempfile.NamedTemporaryFile(dir=directory, delete=False) as output:
            temporary = Path(output.name)
            try:
                with source.extractfile(members[0]) as stream:
                    shutil.copyfileobj(stream, output)
                output.flush()
                if sha(temporary) != record['binary_sha256']:
                    raise ValueError('extracted executable differs from pinned binary digest')
                temporary.chmod(0o700)
            except BaseException:
                temporary.unlink(missing_ok=True); raise
        # Refuse overwrite even if another provisioner won the race.
        try:
            os.link(temporary, target)
        finally:
            temporary.unlink(missing_ok=True)
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--install', nargs='+', choices=sorted(ASSETS), default=sorted(ASSETS))
    parser.add_argument('--directory', type=Path, default=ROOT / 'artifacts/person3/tools')
    args = parser.parse_args()
    if platform.system() != 'Linux' or platform.machine() != 'x86_64':
        parser.error('these pinned assets support Linux x86-64 only')
    directory = args.directory.resolve(); directory.mkdir(parents=True, exist_ok=True)
    results = []
    for name in args.install:
        target = provision(name, directory)
        results.append({'name': name, 'version': ASSETS[name]['version'], 'path': str(target),
                        'sha256': sha(target), 'launch_environment_variable': ASSETS[name]['environment']})
    print(json.dumps({'installed': results, 'host_programs_replaced': False, 'cloud_requests': 0}, indent=2))


if __name__ == '__main__':
    main()
