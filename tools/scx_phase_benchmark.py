#!/usr/bin/env python3
"""Compare Linux, frozen SCX and a phase snapshot only inside a pinned-kernel VM."""
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool-root', required=True, type=Path)
    parser.add_argument('--kernel', type=Path, default=ROOT / 'artifacts/person1/kernel-candidate/arch/x86/boot/bzImage')
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts/person1/scx-phase-comparison')
    args = parser.parse_args(); args.output.mkdir(parents=True, exist_ok=True)
    init = args.output / 'init'
    subprocess.run(['gcc', '-static', '-O2', '-Wall', '-Wextra', '-Werror', '-Wl,--build-id=none',
                    '-o', str(init), str(ROOT / 'tests/resource/phase_guest.c')], check=True)
    files = {'init': (stat.S_IFREG | 0o755, init.read_bytes())}
    for name in ['dev', 'proc', 'sys', 'tmp', 'run', 'run/cerynth']:
        files[name] = (stat.S_IFDIR | 0o755, b'')
    binaries = {'cerynth-scx-baseline': ROOT / 'target/release/cerynth-scx', 'cerynth-scx-phase': ROOT / 'target/release/cerynth-scx-phase'}
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
            '-enable-kvm', '-machine', 'q35', '-cpu', 'host', '-m', '512M', '-smp', '2',
            '-kernel', str(args.kernel), '-initrd', str(initrd), '-append', 'console=ttyS0 panic=-1 rdinit=/init',
            '-no-reboot', '-nographic', '-monitor', 'none', '-nic', 'none'], check=True, timeout=150,
            stdout=output, stderr=subprocess.STDOUT, env=os.environ | {'LD_LIBRARY_PATH': str(tools / 'usr/lib'), 'QEMU_MODULE_DIR': str(tools / 'usr/lib/qemu')})
    text = log.read_text(errors='replace')
    samples = [json.loads(line.split(' ', 1)[1]) for line in text.splitlines() if line.startswith('CERYNTH_PHASE_SAMPLE ')]
    checks = [json.loads(line.split(' ', 1)[1]) for line in text.splitlines() if line.startswith('CERYNTH_CHECK ')]
    results = [json.loads(line.split(' ', 1)[1]) for line in text.splitlines() if line.startswith('CERYNTH_RESULT ')]
    if len(samples) != 15 or not checks or not all(item['passed'] for item in checks) or not results or results[-1]['failures']:
        raise RuntimeError('phase comparison incomplete or failed; inspect ' + str(log))
    summary = {}
    for mode in sorted({item['mode'] for item in samples}):
        values = [item['foreground_ticks'] / (item['foreground_ticks'] + item['background_ticks']) for item in samples if item['mode'] == mode]
        summary[mode] = {'foreground_cpu_share_mean': statistics.mean(values), 'sample_stdev': statistics.stdev(values), 'trials': len(values)}
    report = {'schema_version': 1, 'kind': 'isolated-scx-phase-snapshot-comparison', 'samples': samples, 'summary': summary,
        'checks': checks, 'kernel_sha256': hashlib.file_digest(args.kernel.open('rb'), 'sha256').hexdigest(),
        'binary_sha256': {name: hashlib.file_digest(path.open('rb'), 'sha256').hexdigest() for name, path in binaries.items()},
        'hard_cpu_quota_or_cgroup_weight_guarantee': False, 'host_scheduler_modified': False,
        'limitations': ['synthetic pinned CPU workers, held alive throughout', 'static process snapshot; no PID-reuse guarantee after witnessed workers exit',
                        'experimental scheduler exits after 60 seconds', 'production resource API continues refusing sched_ext', 'CPU service share is not task-completion speedup']}
    (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n'); print(json.dumps(summary))


if __name__ == '__main__': main()
