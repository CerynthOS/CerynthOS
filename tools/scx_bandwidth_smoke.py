#!/usr/bin/env python3
"""Exercise pinned upstream SCX bandwidth only in a disposable kernel VM."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import statistics
import subprocess
from platform_vm_smoke import archive, ROOT
from build_scx_bandwidth_candidate import PIN


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool-root', required=True, type=Path)
    parser.add_argument('--kernel', type=Path, default=ROOT / 'artifacts/person1/kernel-candidate/arch/x86/boot/bzImage')
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts/person1/scx-bandwidth')
    parser.add_argument('--linux-control', action='store_true', help='run identical quota workload under Linux default scheduling')
    args = parser.parse_args(); args.output.mkdir(parents=True, exist_ok=True)
    provenance = json.loads((ROOT / 'artifacts/person1/lavd-source/build.json').read_text())
    candidate = ROOT / 'target/release-fast/scx_lavd'
    if provenance['upstream_revision'] != PIN or provenance['binary_sha256'] != hashlib.file_digest(candidate.open('rb'), 'sha256').hexdigest():
        raise RuntimeError('build the pinned bandwidth candidate before testing it')
    init = args.output / 'init'
    subprocess.run(['gcc', '-static', '-O2', '-Wall', '-Wextra', '-Werror', '-Wl,--build-id=none',
                    '-o', str(init), str(ROOT / 'tests/resource/scx_bandwidth_guest.c')], check=True)
    files = {'init': (stat.S_IFREG | 0o755, init.read_bytes())}
    for name in ['dev', 'proc', 'sys', 'tmp', 'run', 'run/cerynth']:
        files[name] = (stat.S_IFDIR | 0o755, b'')
    kernel_build = args.kernel.resolve().parents[3]
    release = (kernel_build / 'include/config/kernel.release').read_text().strip()
    files['boot/config-' + release] = (stat.S_IFREG | 0o644, (kernel_build / '.config').read_bytes())
    binaries = {'scx_lavd': ROOT / 'target/release-fast/scx_lavd'}
    for name, binary in binaries.items():
        files['usr/lib/cerynth/' + name] = (stat.S_IFREG | 0o755, binary.read_bytes())
        libraries = subprocess.check_output(['ldd', str(binary)], text=True)
        if 'not found' in libraries: raise RuntimeError('unresolved scheduler library')
        for item in re.findall(r'(?:=>\s+)?(/\S+)\s+\(', libraries):
            files[item.lstrip('/')] = (stat.S_IFREG | 0o755, Path(item).read_bytes())
        headers = subprocess.check_output(['readelf', '-l', str(binary)], text=True)
        interpreter = re.search(r'Requesting program interpreter: ([^\]]+)', headers)
        if interpreter:
            item = interpreter.group(1)
            files[item.lstrip('/')] = (stat.S_IFREG | 0o755, Path(item).read_bytes())
    initrd = args.output / 'initramfs.cpio.gz'; initrd.write_bytes(archive(files))
    tools = args.tool_root.resolve(); log = args.output / 'console.log'
    with log.open('wb') as output:
        subprocess.run([str(tools / 'usr/bin/qemu-system-x86_64'), '-L', str(tools / 'usr/share/qemu'),
            '-enable-kvm', '-machine', 'q35', '-cpu', 'host', '-m', '1024M', '-smp', '2',
            '-kernel', str(args.kernel), '-initrd', str(initrd), '-append', 'console=ttyS0 panic=-1 rdinit=/init' + (' cerynth.cpu_policy=linux' if args.linux_control else ''),
            '-no-reboot', '-nographic', '-monitor', 'none', '-nic', 'none'], check=True, timeout=150,
            stdout=output, stderr=subprocess.STDOUT, env=os.environ | {'LD_LIBRARY_PATH': str(tools / 'usr/lib'), 'QEMU_MODULE_DIR': str(tools / 'usr/lib/qemu')})
    text = log.read_text(errors='replace')
    samples = [json.loads(line.split(' ', 1)[1]) for line in text.splitlines() if line.startswith('CERYNTH_BANDWIDTH_SAMPLE ')]
    checks = [json.loads(line.split(' ', 1)[1]) for line in text.splitlines() if line.startswith('CERYNTH_CHECK ')]
    results = [json.loads(line.split(' ', 1)[1]) for line in text.splitlines() if line.startswith('CERYNTH_RESULT ')]
    passed = len(samples) == 9 and bool(checks) and all(item['passed'] for item in checks) and bool(results) and not results[-1]['failures']
    summary = {}
    for mode in sorted({item['condition'] for item in samples}):
        values = [item['parent_cpu_seconds'] / item['wall_seconds'] for item in samples if item['condition'] == mode]
        summary[mode] = {'parent_cpu_fraction_mean': statistics.mean(values), 'sample_stdev': statistics.stdev(values) if len(values) > 1 else None, 'trials': len(values)}
    report = {'schema_version': 1, 'kind': 'isolated-upstream-scx-bandwidth-experiment', 'passed': passed,
        'policy': 'linux-default' if args.linux_control else 'upstream-lavd',
        'upstream_revision': PIN, 'build_provenance': provenance,
        'samples': samples, 'summary': summary, 'checks': checks,
        'kernel_sha256': hashlib.file_digest(args.kernel.open('rb'), 'sha256').hexdigest(),
        'kernel_config_sha256': hashlib.sha256((kernel_build / '.config').read_bytes()).hexdigest(),
        'fixture_sha256': hashlib.sha256((ROOT / 'tests/resource/scx_bandwidth_guest.c').read_bytes()).hexdigest(),
        'binary_sha256': {name: hashlib.file_digest(path.open('rb'), 'sha256').hexdigest() for name, path in binaries.items()},
        'host_scheduler_modified': False, 'production_resource_gate_changed': False,
        'limitations': ['upstream LAVD explicitly labels cpu.max support highly experimental',
                        'VM sustained bandwidth observation; not per-period latency or hard real-time assurance',
                        'does not qualify cpu.weight semantics or the frozen Cerynth scheduler',
                        'production resource API continues refusing sched_ext']}
    (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n'); print(json.dumps(report))
    if not passed:
        raise SystemExit('upstream SCX bandwidth qualification failed; inspect console.log')



if __name__ == '__main__': main()
