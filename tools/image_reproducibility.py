#!/usr/bin/env python3
"""Build two fresh disk images from fixed inputs and compare their actual bytes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def tree_manifest(root):
    """Include source timestamps/modes, symlinks and empty directories in the lock."""
    result = []
    for path in sorted(root.rglob('*')):
        meta = path.lstat()
        item = {'path': str(path.relative_to(root)), 'mode': stat.S_IMODE(meta.st_mode),
                'mtime_ns': meta.st_mtime_ns}
        if path.is_symlink():
            item.update(kind='symlink', target=os.readlink(path))
        elif path.is_file():
            item.update(kind='file', bytes=meta.st_size, sha256=digest(path))
        elif path.is_dir():
            item.update(kind='directory')
        else:
            raise ValueError('unsupported input type: ' + str(path))
        result.append(item)
    return result


def input_lock(tool_root, desktop):
    trees = {'configuration': ROOT / 'packaging/image',
             'overlay': ROOT / ('artifacts/person1/os-image-overlay-desktop' if desktop else 'artifacts/person1/os-image-overlay'),
             'mkosi': tool_root / 'usr/lib/python3.14/site-packages/mkosi'}
    if any(not path.is_dir() for path in trees.values()):
        raise ValueError('prepare the image overlay and local mkosi tools first')
    packages = ROOT / 'artifacts/person1/image-package-cache/cache/pacman/pkg'
    package_records = [{'name': p.name, 'bytes': p.stat().st_size, 'sha256': digest(p)}
                       for p in sorted(packages.glob('*.pkg.tar.*')) if not p.name.endswith(('.sig', '.part'))]
    if not package_records:
        raise ValueError('offline authenticated package cache is empty')
    # mkosi uses a combination of local systemd helpers and the host filesystem tools.
    helpers = {}
    for name in ['systemd-repart', 'ukify', 'mkfs.ext4', 'mkfs.fat', 'mcopy', 'mmd']:
        candidates = [tool_root / 'usr/lib/systemd' / name, tool_root / 'usr/bin' / name,
                      Path('/usr/bin') / name]
        path = next((p for p in candidates if p.is_file()), None)
        if path:
            helpers[name] = {'path': str(path), 'sha256': digest(path)}
    return {'source_revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
            'builder_sha256': digest(ROOT / 'tools/build_os_image.py'),
            'source_date_epoch': 1795190400, 'partition_seed': '826849fe-f108-46bf-bd46-158a22735803',
            'trees': {name: tree_manifest(path) for name, path in trees.items()},
            'packages': package_records, 'helpers': helpers}


def compare_files(left, right):
    """A complete streaming comparison, with bounded diagnostics on differences."""
    left_size, right_size = left.stat().st_size, right.stat().st_size
    differences = []
    mismatched_bytes = 0
    offset = 0
    with left.open('rb') as first, right.open('rb') as second:
        while True:
            a, b = first.read(1024 * 1024), second.read(1024 * 1024)
            if not a and not b:
                break
            if a != b:
                mismatched_bytes += sum(x != y for x, y in zip(a, b)) + abs(len(a) - len(b))
                if len(differences) < 64:
                    first_difference = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
                    differences.append({'block_offset': offset, 'first_difference': offset + first_difference})
            offset += max(len(a), len(b))
    return {'byte_identical': not mismatched_bytes and left_size == right_size,
            'left_bytes': left_size, 'right_bytes': right_size,
            'left_sha256': digest(left), 'right_sha256': digest(right),
            'mismatched_bytes': mismatched_bytes, 'first_mismatching_blocks': differences}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool-root', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts/person1/image-reproducibility')
    parser.add_argument('--desktop', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    tool_root = args.tool_root.resolve()
    lock = input_lock(tool_root, args.desktop)
    (output / 'inputs.json').write_text(json.dumps(lock, indent=2) + '\n')
    trials = []
    for index in range(2):
        destination = output / f'build-{index + 1}'
        workspace = output / f'workspace-{index + 1}'
        if destination.exists() or workspace.exists():
            raise ValueError('independent build destination must be new: ' + str(destination))
        command = [sys.executable, str(ROOT / 'tools/build_os_image.py'), '--tool-root', str(tool_root),
                   '--offline', '--disk-only', '--output-directory', str(destination),
                   '--workspace-directory', str(workspace)] + (['--desktop'] if args.desktop else [])
        start = time.monotonic()
        with (output / f'build-{index + 1}.log').open('w') as stream:
            subprocess.run(command, check=True, stdout=stream, stderr=subprocess.STDOUT, cwd=ROOT)
        if input_lock(tool_root, args.desktop) != lock:
            raise RuntimeError('image inputs changed during qualification; comparison is invalid')
        trials.append({'independent_build': index + 1, 'seconds': time.monotonic() - start,
                       'image': str(destination / 'cerynthos.raw')})
        print(json.dumps(trials[-1]), flush=True)
    comparison = compare_files(*(Path(item['image']) for item in trials))
    report = {'schema_version': 1, 'kind': 'independent-full-disk-rebuild-comparison',
              'variant': 'desktop' if args.desktop else 'base', 'trials': trials, 'comparison': comparison,
              'input_lock_sha256': digest(output / 'inputs.json'), 'inputs_unchanged': True,
              'fresh_build_roots': True, 'shared_authenticated_package_cache': True,
              'host_block_devices_written': False,
              'limitations': ['same host and toolchain; not cross-machine reproducibility',
                              'prepared payload frozen; not an independent rebuild of every upstream package or Rust binary']}
    (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report), flush=True)
    if not comparison['byte_identical']:
        raise SystemExit('disk rebuild differs; inspect report and preserve both images for diagnosis')


if __name__ == '__main__':
    main()
