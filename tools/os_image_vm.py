#!/usr/bin/env python3
"""Boot a disk through UEFI/KVM; only disposable overlays and keys are used."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import shlex
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


class ImageVM:
    def __init__(self, tools, image, key, work, desktop=False):
        self.tools, self.image, self.key, self.work = map(lambda p: Path(p).resolve(), (tools, image, key, work))
        self.env = os.environ | {'LD_LIBRARY_PATH': str(self.tools / 'usr/lib'),
                                 'QEMU_MODULE_DIR': str(self.tools / 'usr/lib/qemu')}
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            self.port = listener.getsockname()[1]
        self.process = None
        self.log = None
        self.desktop = desktop

    def ssh(self, command, check=True, timeout=120):
        result = subprocess.run(['ssh', '-i', str(self.key), '-p', str(self.port),
                               '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=2',
                               '-o', 'ServerAliveInterval=2', '-o', 'ServerAliveCountMax=2',
                               '-o', 'StrictHostKeyChecking=accept-new',
                               '-o', 'UserKnownHostsFile=' + str(self.work / 'known_hosts'),
                               'root@127.0.0.1', command], capture_output=True, text=True,
                              timeout=timeout, check=False)
        if check and result.returncode:
            raise RuntimeError('guest command failed: ' + result.stdout[-8192:] + result.stderr[-8192:])
        return result

    def upload(self, source, destination):
        payload = base64.b64encode(Path(source).read_bytes()).decode()
        code = f'import base64;open({str(destination)!r},"wb").write(base64.b64decode({payload!r}))'
        self.ssh('python3 -c ' + shlex.quote(code))

    def copy_to(self, source, destination):
        subprocess.run(['scp', '-q', '-r', '-i', str(self.key), '-P', str(self.port),
            '-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=accept-new',
            '-o', 'UserKnownHostsFile=' + str(self.work / 'known_hosts'),
            str(Path(source).resolve()), 'root@127.0.0.1:' + destination], check=True, timeout=240)

    def qmp(self, command, arguments):
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(15); connection.connect(str(self.work / 'qmp.sock'))
            stream = connection.makefile('rwb')
            json.loads(stream.readline())
            for index, (name, params) in enumerate([('qmp_capabilities', {}), (command, arguments)]):
                stream.write(json.dumps({'execute': name, 'arguments': params, 'id': index}).encode() + b'\n'); stream.flush()
                while True:
                    response = json.loads(stream.readline())
                    if response.get('id') == index:
                        if 'error' in response: raise RuntimeError(response['error'])
                        break
            return response['return']

    def start(self):
        self.work.mkdir(parents=True, exist_ok=True)
        disk = self.work / 'guest.qcow2'
        if not disk.exists():
            subprocess.run([str(self.tools / 'usr/bin/qemu-img'), 'create', '-q', '-f', 'qcow2',
                            '-F', 'raw', '-b', str(self.image), str(disk)], check=True, env=self.env)
        variables = self.work / 'OVMF_VARS.fd'
        if not variables.exists():
            shutil.copyfile(self.tools / 'usr/share/edk2/x64/OVMF_VARS.4m.fd', variables)
        self.log = (self.work / 'serial.log').open('a')
        self.process = subprocess.Popen([str(self.tools / 'usr/bin/qemu-system-x86_64'),
            '-L', str(self.tools / 'usr/share/qemu'), '-enable-kvm', '-machine', 'q35',
            '-cpu', 'host', '-m', '3072' if self.desktop else '2048', '-smp', '2', '-display', 'none',
            '-vga', 'std',
            '-drive', 'if=pflash,format=raw,readonly=on,file=' + str(self.tools / 'usr/share/edk2/x64/OVMF_CODE.4m.fd'),
            '-drive', 'if=pflash,format=raw,file=' + str(variables),
            '-drive', 'if=virtio,format=qcow2,discard=unmap,detect-zeroes=unmap,file=' + str(disk),
            '-device', 'virtio-rng-pci', '-netdev', f'user,id=net,restrict=on,hostfwd=tcp:127.0.0.1:{self.port}-:22',
            '-device', 'virtio-net-pci,netdev=net', '-serial', 'stdio', '-monitor', 'none',
            '-qmp', 'unix:' + str(self.work / 'qmp.sock') + ',server=on,wait=off'],
            stdout=self.log, stderr=subprocess.STDOUT, env=self.env)
        deadline = time.monotonic() + 150
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError('VM exited; inspect ' + str(self.work / 'serial.log'))
            if self.ssh('true', check=False, timeout=5).returncode == 0:
                return
            time.sleep(1)
        raise TimeoutError('UEFI image SSH readiness deadline; inspect ' + str(self.work / 'serial.log'))

    def stop(self, power_cut=False):
        if self.process and self.process.poll() is None:
            if not power_cut:
                try:
                    self.ssh('systemctl poweroff', check=False, timeout=5)
                except (subprocess.TimeoutExpired, OSError):
                    self.process.terminate()
                try:
                    self.process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    self.process.terminate()
            else:
                self.process.kill()
            self.process.wait(timeout=10)
        if self.log:
            self.log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool-root', required=True, type=Path)
    parser.add_argument('--image', type=Path, default=ROOT / 'artifacts/person1/os-image/cerynthos.raw')
    parser.add_argument('--key', type=Path, default=ROOT / 'artifacts/person1/image-access/id_ed25519')
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts/person1/os-image-boot.json')
    parser.add_argument('--desktop', action='store_true')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='cerynth-image-vm-', dir='/var/tmp') as directory:
        work = Path(directory)
        vm = ImageVM(args.tool_root, args.image, args.key, work, desktop=args.desktop)
        try:
            vm.start()
            deadline = time.monotonic() + 40
            while True:
                info = vm.ssh('uname -r; systemctl is-active cerynthd; systemctl is-active user@1000; '
                              'runuser -u cerynth -- env XDG_RUNTIME_DIR=/run/user/1000 systemctl --user is-active cerynth-agentd cerynth-inferenced', check=False).stdout
                if len([line for line in info.splitlines() if line == 'active']) == 4: break
                if time.monotonic() > deadline: raise RuntimeError(info)
                time.sleep(.5)
            environment = 'runuser -u cerynth -- env TMPDIR=/var/tmp XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus '
            harness = ROOT / 'tools/advanced_resources_smoke.py'
            vm.upload(harness, '/usr/share/cerynth/validation/advanced_resources_smoke.py')
            command = environment + 'python3 /usr/share/cerynth/validation/advanced_resources_smoke.py --binary /usr/lib/cerynth/cerynth-agentd --require-advanced --output /var/lib/cerynth-validation/advanced.json'
            advanced = json.loads(vm.ssh(command, timeout=180).stdout.strip().splitlines()[-1])
            desktop = None
            if args.desktop:
                deadline = time.monotonic() + 75
                while vm.ssh("pgrep -u 1000 -x kwin_wayland; pgrep -u 1000 -x plasmashell; pgrep -u 1000 -f '^(/usr/bin/)?cerynth-observer$'", check=False).returncode:
                    if time.monotonic() > deadline:
                        raise RuntimeError(vm.ssh('journalctl -b --no-pager -n 100').stdout)
                    time.sleep(1)
                ui_harness = ROOT / 'tools/desktop_responsiveness_smoke.py'
                vm.upload(ui_harness, '/usr/share/cerynth/validation/desktop_responsiveness_smoke.py')
                desktop = json.loads(vm.ssh(environment + 'python3 /usr/share/cerynth/validation/desktop_responsiveness_smoke.py --output /var/lib/cerynth-validation/desktop.json').stdout.strip().splitlines()[-1])
                vm.qmp('screendump', {'filename': str(args.output.with_suffix('.ppm').resolve())})
            # fsync a marker, cut power, boot the same overlay, verify persistence.
            vm.ssh("python3 -c 'import os; f=open(\"/var/lib/cerynth-validation/recovery-marker\",\"w\"); f.write(\"persistent\"); f.flush(); os.fsync(f.fileno()); os.sync()'")
            vm.stop(power_cut=True)
            vm.start()
            assert vm.ssh('cat /var/lib/cerynth-validation/recovery-marker').stdout == 'persistent'
            report = {'schema_version': 1, 'kind': 'development-os-uefi-kvm-boot',
                      'image_sha256': hashlib.file_digest(args.image.open('rb'), 'sha256').hexdigest(),
                      'qualification_harness_sha256': hashlib.sha256(harness.read_bytes()).hexdigest(),
                      'kernel_and_services': info.splitlines(), 'advanced_resources': advanced,
                      'desktop': desktop,
                      'fsynced_marker_survived_power_cut': True, 'physical_hardware_qualified': False,
                      'secure_boot_qualified': False, 'guest_network_external_access': False}
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(report))
        finally:
            try:
                vm.stop()
            finally:
                shutil.copyfile(work / 'serial.log', args.output.with_suffix('.serial.log'))


if __name__ == '__main__':
    main()
