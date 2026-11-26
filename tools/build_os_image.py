#!/usr/bin/env python3
"""Build a pinned development disk image using workspace-local mkosi tools."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool-root', type=Path, required=True)
    parser.add_argument('--desktop', action='store_true')
    parser.add_argument('--ab', action='store_true', help='build the explicitly partitioned development A/B variant')
    parser.add_argument('--force', action='store_true')
    parser.add_argument('--offline', action='store_true', help='require previously populated metadata and authenticated package cache')
    parser.add_argument('--output-directory', type=Path, help='separate destination for an independent build')
    parser.add_argument('--workspace-directory', type=Path, help='separate temporary build root')
    parser.add_argument('--disk-only', action='store_true', help='omit split kernel/initrd/UKI copies')
    args = parser.parse_args()
    if args.desktop and args.ab:
        parser.error('--desktop and --ab are separate image variants')
    tools = args.tool_root.resolve()
    site = tools / 'usr/lib/python3.14/site-packages'
    shim = ROOT / 'artifacts/person1/mkosi-python'
    shim.mkdir(parents=True, exist_ok=True)
    # mkosi starts Python again in its filesystem sandbox. Bind the complete
    # pure-Python PE parser through ExtraSearchPaths, without host installation.
    shutil.copyfile('/usr/bin/python', shim / 'mkosi-python')
    (shim / 'mkosi-python').chmod(0o755)
    shutil.copyfile(site / 'pefile.py', shim / 'pefile.py')
    shutil.copytree(site / 'ordlookup', shim / 'ordlookup', dirs_exist_ok=True)
    # The sandbox locates filesystem helpers by PATH; expose only the needed
    # verified executable rather than replacing the host tool search wholesale.
    shutil.copyfile(tools / 'usr/bin/mkfs.fat', shim / 'mkfs.vfat')
    (shim / 'mkfs.vfat').chmod(0o755)
    for name in ['mcopy', 'mmd']:
        shutil.copyfile(tools / 'usr/bin' / name, shim / name)
        (shim / name).chmod(0o755)
    (shim / 'python3').write_text('#!/bin/sh\nexport PYTHONPATH="$(dirname "$0")"\nexec /usr/bin/python "$@"\n')
    (shim / 'python3').chmod(0o755)
    command = [str(shim / 'mkosi-python'), str(tools / 'usr/bin/mkosi'), '-C', str(ROOT / 'packaging/image'),
               '--extra-search-path=' + str(shim), '--extra-search-path=' + str(tools / 'usr/lib/systemd'),
               '--cache-directory=' + str(ROOT / 'artifacts/person1/image-cache'),
               '--workspace-directory=' + str((args.workspace_directory or ROOT / 'artifacts/person1/image-work').resolve()),
               '--package-cache-dir=' + str(ROOT / 'artifacts/person1/image-package-cache'),
               '--source-date-epoch=1795190400']
    if args.desktop:
        command += ['--profile=desktop']
    if args.ab:
        command += ['--profile=ab']
    name = 'os-image-ab' if args.ab else 'os-image-desktop' if args.desktop else 'os-image'
    output = (args.output_directory or ROOT / 'artifacts/person1' / name).resolve()
    command += ['--output-directory=' + str(output)]
    if args.disk_only:
        command += ['--split-artifacts=']
    if args.force:
        command += ['--force']
    if args.offline:
        command += ['--cache-only=always']
    subprocess.run(command + ['build'], check=True, cwd=ROOT, env=os.environ | {'PYTHONPATH': str(site)})
    print(json.dumps({'development_image': True, 'desktop': args.desktop, 'ab': args.ab,
                      'output_directory': str(output)}))


if __name__ == '__main__':
    main()
