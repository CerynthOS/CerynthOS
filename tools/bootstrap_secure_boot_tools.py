#!/usr/bin/env python3
"""Build pinned signer/verifier tools privately and install authenticated OVMF editing helpers."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
SOURCE = 'https://git.kernel.org/pub/scm/linux/kernel/git/jejb/sbsigntools.git'
REVISION = 'db731f0c4dd75e112e2abdfbe9443065742e58c3'
CCAN = 'b1f28e17227f2320d07fe052a8a48942fe17caa5'


def run(args, cwd=None, **kwargs):
    return subprocess.run([str(v) for v in args], cwd=cwd, check=True, text=True, capture_output=True, **kwargs).stdout


def sha(path):
    with path.open('rb') as stream: return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, default=ROOT / 'artifacts/person1/qemu-local')
    parser.add_argument('--skip-packages', action='store_true', help='use already installed virt-firmware and its Python dependencies')
    args = parser.parse_args(); directory = args.directory.resolve()
    if not args.skip_packages:
        run([sys.executable, ROOT / 'tools/bootstrap_platform_qemu.py', '--output', directory,
             '--packages-only', 'virt-firmware', 'python-cryptography', 'python-crypt_r', 'python-pefile', '--skip-dependency-resolution'])
    source = directory.parent / 'sbsigntools-source'
    if not source.exists():
        source.mkdir(parents=True)
        run(['git', 'init', source])
        run(['git', 'remote', 'add', 'origin', SOURCE], cwd=source)
        run(['git', '-c', 'credential.helper=', '-c', 'core.askPass=', 'fetch', '--depth', '1', 'origin', REVISION], cwd=source, timeout=120)
        run(['git', 'checkout', '--detach', REVISION], cwd=source)
    if run(['git', 'rev-parse', 'HEAD'], cwd=source).strip() != REVISION:
        raise ValueError('sbsigntools checkout differs from the pinned revision')
    run(['git', '-c', 'credential.helper=', '-c', 'core.askPass=',
         '-c', 'submodule.lib/ccan.git.url=https://github.com/rustyrussell/ccan.git',
         'submodule', 'update', '--init', '--depth', '1'], cwd=source, timeout=120)
    if run(['git', 'rev-parse', 'HEAD'], cwd=source / 'lib/ccan.git').strip() != CCAN:
        raise ValueError('ccan source differs from the upstream submodule pin')
    changed = run(['git', 'diff', 'HEAD', '--name-only'], cwd=source).splitlines()
    if any(name != 'src/image.c' for name in changed):
        raise ValueError('unexpected tracked signer source modifications')
    if run(['git', 'status', '--porcelain', '--untracked-files=no'], cwd=source / 'lib/ccan.git').strip():
        raise ValueError('ccan tracked source is modified')
    original = run(['git', 'show', REVISION + ':src/image.c'], cwd=source)
    # GCC diagnoses an upstream diagnostic-only counter under -Werror. Mark
    # it used; retain every hash/signature operation and all compiler checks.
    patched = original.replace('rc = SHA256_Final(digest, &ctx);\n\n\treturn !rc;',
        'rc = SHA256_Final(digest, &ctx);\n\t(void)n; /* diagnostic-only byte counter */\n\n\treturn !rc;')
    image = source / 'src/image.c'
    if image.read_text() not in [original, patched]: raise ValueError('unexpected signer source modifications')
    image.write_text(patched)
    run(['bash', './autogen.sh'], cwd=source, timeout=120)
    # The selected programs do not link GNU-EFI. CRTPATH is a configure input
    # for the unbuilt EFI test programs; those tests are not claimed here.
    import os
    run(['./configure', '--prefix=' + str(directory / 'root/usr')], cwd=source,
        env=os.environ | {'CRTPATH': str(source)}, timeout=120)
    run(['make', '-C', 'lib/ccan', '-j2'], cwd=source, timeout=120)
    run(['make', '-C', 'src', '-j2', 'sbsign', 'sbverify', 'sbattach'], cwd=source, timeout=120)
    target = directory / 'root/usr/bin'; target.mkdir(parents=True, exist_ok=True)
    records = []
    for name in ['sbsign', 'sbverify', 'sbattach']:
        temporary = target / (name + '.new')
        shutil.copyfile(source / 'src' / name, temporary); temporary.chmod(0o755); temporary.replace(target / name)
        records.append({'name': name, 'sha256': sha(target / name)})
    report = {'kind': 'workspace-local-secure-boot-tools', 'source_url': SOURCE, 'source_revision': REVISION,
              'ccan_revision': CCAN, 'diagnostic_counter_only_patch': True, 'image_source_sha256': sha(image),
              'binaries': records, 'host_packages_installed': False, 'physical_firmware_modified': False}
    (directory / 'secure-boot-tools.json').write_text(json.dumps(report, indent=2) + '\n'); print(json.dumps(report))


if __name__ == '__main__': main()
