#!/usr/bin/env python3
"""Configure swap only inside a disposable UEFI guest and qualify the runtime API."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
from os_image_vm import ImageVM, ROOT


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool-root', type=Path, required=True)
    parser.add_argument('--image', type=Path, default=ROOT / 'artifacts/person1/os-image-ab/cerynthos.raw')
    parser.add_argument('--key', type=Path, default=ROOT / 'artifacts/person1/image-access/id_ed25519')
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts/person1/active-swap-vm.json')
    args = parser.parse_args(); args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='cerynth-swap-vm-', dir='/var/tmp') as temporary:
        work = Path(temporary); vm = ImageVM(args.tool_root, args.image, args.key, work)
        try:
            vm.start()
            swap = '/var/lib/cerynth-validation/qualification.swap'
            vm.ssh('set -eu\nfallocate -l 256M ' + swap + '\nchmod 600 ' + swap + '\nmkswap ' + swap + '\nswapon ' + swap)
            vm.ssh('if test -w /sys/module/zswap/parameters/enabled; then printf N > /sys/module/zswap/parameters/enabled; fi')
            before = int(vm.ssh("awk '$1 == \"pswpout\" {print $2}' /proc/vmstat").stdout.strip())
            vm.upload(ROOT / 'tools/active_swap_smoke.py', '/usr/share/cerynth/validation/active_swap_smoke.py')
            command = 'runuser -u cerynth -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus python3 /usr/share/cerynth/validation/active_swap_smoke.py --binary /usr/lib/cerynth/cerynth-agentd --output /var/lib/cerynth-validation/active-swap.json'
            fixture = json.loads(vm.ssh(command, timeout=180).stdout.strip().splitlines()[-1])
            after = int(vm.ssh("awk '$1 == \"pswpout\" {print $2}' /proc/vmstat").stdout.strip())
            assert after > before, 'no pages reached the swap device'
            kernel = vm.ssh('journalctl -k -b --no-pager -o cat').stdout
            evidence = [line for line in kernel.splitlines() if 'CONSTRAINT_MEMCG' in line and fixture['no_swap_resource_slice'] in line]
            assert evidence, 'missing specific cgroup OOM evidence for the zero-swap control'
            assert vm.ssh('systemctl is-active cerynthd user@1000').stdout.splitlines() == ['active', 'active']
            vm.ssh('swapoff ' + swap, timeout=60)
            assert swap not in vm.ssh('cat /proc/swaps').stdout
            vm.ssh('rm ' + swap)
            result = {'schema_version': 1, 'kind': 'real-active-swap-uefi-vm', 'fixture': fixture,
                'swapout_pages_delta': after - before, 'no_swap_cgroup_oom_evidence': evidence,
                'core_services_survived': True, 'guest_swap_removed': True, 'host_swap_modified': False,
                'physical_hardware_qualified': False,
                'image_sha256': hashlib.file_digest(args.image.open('rb'), 'sha256').hexdigest(),
                'fixture_sha256': hashlib.sha256((ROOT / 'tools/active_swap_smoke.py').read_bytes()).hexdigest()}
            args.output.write_text(json.dumps(result, indent=2) + '\n'); print(json.dumps(result))
        finally:
            try: vm.stop()
            finally:
                if (work / 'serial.log').exists(): shutil.copyfile(work / 'serial.log', args.output.with_suffix('.serial.log'))


if __name__ == '__main__': main()
