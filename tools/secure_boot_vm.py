#!/usr/bin/env python3
"""Exercise real UEFI signature decisions with throwaway keys and private OVMF stores."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import time
import uuid
from platform_vm_smoke import archive

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def run(command, env=None):
    result = subprocess.run([str(a) for a in command], check=False, capture_output=True, text=True, timeout=60, env=env)
    if result.returncode:
        raise RuntimeError('tool failed: ' + str(command[0]) + '\n' + result.stderr[-3000:])
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool-root', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts/person1/secure-boot-vm')
    parser.add_argument('--kernel', type=Path, default=ROOT / 'artifacts/person1/os-image/cerynthos.vmlinuz')
    parser.add_argument('--stub', type=Path, default=Path('/usr/lib/systemd/boot/efi/linuxx64.efi.stub'))
    args = parser.parse_args()
    tools = args.tool_root.resolve(); output = args.output.resolve()
    if not args.kernel.is_file() or not args.stub.is_file():
        parser.error('supply an EFI-capable kernel and installed systemd EFI stub')
    output.mkdir(parents=True, mode=0o700, exist_ok=True); output.chmod(0o700)
    site = tools / 'usr/lib/python3.14/site-packages'
    env = os.environ | {'PYTHONPATH': str(site), 'LD_LIBRARY_PATH': str(tools / 'usr/lib'), 'QEMU_MODULE_DIR': str(tools / 'usr/lib/qemu')}
    keys = output / 'throwaway-keys'
    keys.mkdir(mode=0o700, exist_ok=True)
    if keys.stat().st_uid != os.getuid() or stat.S_IMODE(keys.stat().st_mode) != 0o700:
        raise ValueError('test key directory must be private and owned')
    for name in ['PK', 'KEK', 'probe', 'candidate', 'unauthorized']:
        key, certificate = keys / (name + '.key'), keys / (name + '.crt')
        if not key.exists() and not certificate.exists():
            old_mask = os.umask(0o077)
            try:
                run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', key, '-out', certificate,
                     '-days', '3', '-sha256', '-subj', '/CN=CerynthOS disposable VM test ' + name])
            finally:
                os.umask(old_mask)
        if not key.is_file() or not certificate.is_file() or stat.S_IMODE(key.stat().st_mode) != 0o600:
            raise ValueError('test key/certificate pair missing or not private')
        if 'CerynthOS disposable VM test' not in run(['openssl', 'x509', '-in', certificate, '-noout', '-subject']):
            raise ValueError('this runner accepts its own disposable test identities only')
    probe, init = output / 'probe.efi', output / 'init'
    run(['gcc', '-c', '-O1', '-ffreestanding', '-fno-builtin', '-fno-stack-protector', '-fshort-wchar', '-mno-red-zone',
         '-o', output / 'probe.o', ROOT / 'tests/secure_boot/probe.c'])
    run(['objcopy', '--remove-section=.comment', '--remove-section=.note.GNU-stack', output / 'probe.o'])
    run(['ld', '-mi386pep', '--subsystem', '10', '--entry', 'efi_main', '--image-base', '0x100000', '--enable-reloc-section',
         '-o', probe, output / 'probe.o'])
    run(['gcc', '-static', '-O2', '-Wall', '-Wextra', '-Werror', '-Wl,--build-id=none', '-o', init, ROOT / 'tests/secure_boot/init.c'])
    files = {'init': (stat.S_IFREG | 0o755, init.read_bytes())}
    files.update({p: (stat.S_IFDIR | 0o755, b'') for p in ['proc', 'sys', 'dev', 'tmp', 'run']})
    initrd = output / 'initrd.cpio.gz'; initrd.write_bytes(archive(files))
    release = output / 'os-release'; release.write_text('ID=cerynth-secureboot-test\nNAME=CerynthOS VM test\n')
    unsigned = output / 'unsigned.efi'
    run([sys.executable, tools / 'usr/bin/ukify', 'build', '--linux', args.kernel.resolve(), '--initrd', initrd,
         '--cmdline', 'console=ttyS0 panic=-1 rdinit=/init', '--os-release', '@' + str(release),
         '--uname', 'cerynth-secureboot-test', '--stub', args.stub.resolve(), '--output', unsigned], env)
    signer = Path('/usr/lib/systemd/systemd-sbsign')
    if not signer.is_file():
        raise ValueError('install systemd-sbsign before running the disposable VM matrix')
    def sign(source, name, target):
        run([signer, '--private-key=' + str(keys / (name + '.key')), '--certificate=' + str(keys / (name + '.crt')),
             '--output=' + str(target), 'sign', source])
    trusted = output / 'trusted.efi'; untrusted = output / 'unauthorized.efi'; signed_probe = output / 'probe-signed.efi'
    sign(unsigned, 'candidate', trusted); sign(unsigned, 'unauthorized', untrusted); sign(probe, 'probe', signed_probe)
    tampered = output / 'tampered.efi'; shutil.copyfile(trusted, tampered)
    with tampered.open('r+b') as stream:
        stream.seek(4096); value = stream.read(1); stream.seek(4096); stream.write(bytes([value[0] ^ 1]))
    owner = str(uuid.uuid4())
    firmware = tools / 'usr/share/edk2/x64/OVMF_CODE.secboot.4m.fd'
    template = tools / 'usr/share/edk2/x64/OVMF_VARS.4m.fd'
    records = []
    for name, candidate in [('signed', trusted), ('unsigned', unsigned), ('unauthorized', untrusted),
                            ('tampered', tampered), ('revoked', trusted), ('failed-update-fallback', unsigned)]:
        work = output / name; work.mkdir(exist_ok=True)
        variables = work / 'OVMF_VARS.fd'
        command = [sys.executable, tools / 'usr/bin/virt-fw-vars', '--input', template, '--output', variables,
                   '--set-pk', owner, keys / 'PK.crt', '--add-kek', owner, keys / 'KEK.crt',
                   '--add-db', owner, keys / 'probe.crt', '--add-db', owner, keys / 'candidate.crt',
                   '--no-microsoft', '--microsoft-kek', 'none', '--sb']
        if name == 'revoked': command += ['--add-dbx-cert', owner, keys / 'candidate.crt']
        run(command, env)
        disk = work / 'esp.raw'
        with disk.open('wb') as stream: stream.truncate(64 * 1024**2)
        run([tools / 'usr/bin/mkfs.fat', '-F', '32', disk], env)
        for directory in ['::/EFI', '::/EFI/BOOT']:
            run([tools / 'usr/bin/mmd', '-i', disk, directory], env)
        for source, destination in [(signed_probe, '::/EFI/BOOT/BOOTX64.EFI'), (candidate, '::/candidate.efi')]:
            run([tools / 'usr/bin/mcopy', '-i', disk, source, destination], env)
        if name == 'failed-update-fallback': run([tools / 'usr/bin/mcopy', '-i', disk, trusted, '::/fallback.efi'], env)
        console = work / 'serial.log'; started = time.monotonic(); timed_out = False
        with console.open('wb') as log:
            process = subprocess.Popen([str(tools / 'usr/bin/qemu-system-x86_64'), '-L', str(tools / 'usr/share/qemu'),
                '-enable-kvm', '-machine', 'q35,smm=on', '-cpu', 'host', '-m', '768M', '-smp', '2',
                '-global', 'driver=cfi.pflash01,property=secure,value=on', '-global', 'ICH9-LPC.disable_s3=1',
                '-drive', 'if=pflash,format=raw,unit=0,readonly=on,file=' + str(firmware),
                '-drive', 'if=pflash,format=raw,unit=1,file=' + str(variables),
                '-drive', 'if=virtio,format=raw,file=' + str(disk), '-nic', 'none', '-display', 'none', '-vga', 'std',
                '-serial', 'stdio', '-monitor', 'none', '-no-reboot'], stdout=log, stderr=subprocess.STDOUT, env=env)
            try:
                process.wait(timeout=45)
            except subprocess.TimeoutExpired:
                timed_out = True; process.kill(); process.wait(timeout=10)
        text = console.read_text(errors='replace')
        secure = 'CERYNTH_UEFI secure_boot=0000000000000001 setup_mode=0000000000000000' in text
        kernel = 'CERYNTH_SECURE_BOOT_KERNEL_OK' in text
        accepted = 'CERYNTH_LOAD primary status=0000000000000000' in text
        security_violation = 'CERYNTH_LOAD primary status=800000000000001a' in text
        access_denied = 'CERYNTH_LOAD primary status=800000000000000f' in text
        rejected = security_violation or access_denied
        fallback = 'CERYNTH_LOAD fallback status=0000000000000000' in text
        passed = secure and not timed_out and ((accepted and kernel) if name == 'signed' else
            (rejected and fallback and kernel) if name == 'failed-update-fallback' else (rejected and not kernel))
        item = {'case': name, 'passed': passed, 'firmware_secure_boot_enabled': secure, 'primary_accepted': accepted,
                'primary_security_violation': security_violation, 'primary_access_denied': access_denied,
                'primary_authentication_rejected': rejected, 'kernel_booted': kernel, 'fallback_loaded': fallback,
                'timeout': timed_out, 'seconds': time.monotonic() - started, 'candidate_sha256': digest(candidate),
                'varstore_after_sha256': digest(variables), 'console_sha256': digest(console)}
        records.append(item); print(json.dumps(item), flush=True)
    report = {'schema_version': 1, 'kind': 'real-ovmf-secure-boot-signature-matrix', 'cases': records,
              'passed': all(r['passed'] for r in records), 'kernel_sha256': digest(args.kernel), 'efi_stub_sha256': digest(args.stub),
              'firmware_sha256': digest(firmware), 'signed_uki_sha256': digest(trusted),
              'signer_version': run([signer, '--version']).splitlines()[0], 'test_keys_only': True,
              'host_firmware_or_block_devices_modified': False, 'production_signing_identity_selected': False,
              'limitations': ['disposable OVMF variable stores only; not physical enrollment',
                              'tiny signed-UKI kernel appliance; not full desktop image Secure Boot qualification',
                              'fallback is the test loader path; not an integrated production A/B Secure Boot update',
                              'no verified-root or root-filesystem integrity claim']}
    (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    if not report['passed']: raise SystemExit('Secure Boot matrix failed; inspect retained serial logs')


if __name__ == '__main__':
    main()
