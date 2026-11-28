#!/usr/bin/env python3
"""Read-only hardware/driver observations; never installs, reboots or claims acceptance."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import time


def read(path):
    try: return Path(path).read_text().strip()
    except OSError: return None


def command(arguments):
    try:
        result = subprocess.run(arguments, capture_output=True, text=True, timeout=10,
                                env={'PATH': '/usr/bin:/bin', 'LANG': 'C'})
        return {'returncode': result.returncode, 'stdout': result.stdout[:65536]}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {'unavailable': type(error).__name__}


def probe(expected_overlay=None):
    virtualization = command(['systemd-detect-virt'])
    kind = virtualization.get('stdout', '').strip() or 'unknown'
    graphics = []
    for device in sorted(Path('/sys/bus/pci/devices').glob('*')):
        if not (read(device / 'class') or '').startswith('0x03'): continue
        driver = device / 'driver'
        graphics.append({'pci_address': device.name, 'vendor': read(device / 'vendor'),
                         'device': read(device / 'device'),
                         'driver': driver.resolve().name if driver.exists() else None})
    connectors = []
    for path in sorted(Path('/sys/class/drm').glob('card*-*/status')):
        connectors.append({'connector': path.parent.name, 'status': read(path),
                           'modes': (read(path.parent / 'modes') or '').splitlines()})
    cpu_models = sorted({line.split(':', 1)[1].strip() for line in (read('/proc/cpuinfo') or '').splitlines()
                         if line.startswith('model name')})
    overlay = Path('/usr/share/doc/cerynth/image-overlay.json')
    actual_overlay = hashlib.sha256(overlay.read_bytes()).hexdigest() if overlay.is_file() else None
    boot = {'efi': Path('/sys/firmware/efi').is_dir(), 'root_mount': None}
    result = command(['findmnt', '--json', '--output', 'FSTYPE,OPTIONS', '/'])
    if result.get('returncode') == 0:
        boot['root_mount'] = json.loads(result['stdout'])['filesystems']
    return {'schema_version': 1, 'kind': 'read-only-platform-hardware-observation',
        'observed_at_ms': time.time_ns() // 1_000_000, 'architecture': platform.machine(),
        'kernel': platform.release(), 'virtualization': kind,
        'physical_host_observed': kind == 'none' and virtualization.get('returncode') == 1,
        'cpu': {'models': cpu_models, 'logical': os.cpu_count(), 'allowed': sorted(os.sched_getaffinity(0))},
        'memory_total_kib': next((int(line.split()[1]) for line in (read('/proc/meminfo') or '').splitlines()
                                  if line.startswith('MemTotal:')), None),
        'system': {'vendor': read('/sys/class/dmi/id/sys_vendor'), 'product': read('/sys/class/dmi/id/product_name')},
        'graphics': graphics, 'display_connectors': connectors,
        'nvidia': command(['nvidia-smi', '--query-gpu=name,driver_version,memory.total', '--format=csv,noheader,nounits']) if shutil.which('nvidia-smi') else None,
        'cgroup_v2': Path('/sys/fs/cgroup/cgroup.controllers').exists(),
        'available_controllers': (read('/sys/fs/cgroup/cgroup.controllers') or '').split(),
        'boot': boot, 'image_overlay_sha256': actual_overlay,
        'expected_image_overlay_matches': expected_overlay is not None and actual_overlay == expected_overlay,
        'installation_or_reboot_performed': False, 'physical_boot_acceptance': False,
        'limitations': ['An observation on the development host does not qualify a different OS image',
                        'Physical installation, reboot/power-loss and workload evidence require an authorized test target',
                        'Driver binding and connected outputs do not prove rendering correctness or responsiveness']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--expected-overlay-sha256')
    args = parser.parse_args()
    value = probe(args.expected_overlay_sha256)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(value, indent=2) + '\n')
    print(json.dumps({'output': str(args.output), 'kernel': value['kernel'], 'virtualization': value['virtualization'],
                      'graphics': value['graphics'], 'image_matches': value['expected_image_overlay_matches'],
                      'physical_boot_acceptance': False}))


if __name__ == '__main__': main()
