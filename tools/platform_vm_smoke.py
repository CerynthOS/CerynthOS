#!/usr/bin/env python3
"""Build a deterministic initramfs and test the pinned kernel in a bounded VM.

This is a kernel/resource test appliance, not a desktop distribution image.
The host scheduler, cgroups and installed packages are never modified.
"""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import re
import stat
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def archive(files):
    """newc, sorted names, fixed uid/gid/mtime/inodes, no host metadata."""
    result = bytearray()
    entries = dict(files)
    for name in list(entries):
        parent = Path(name).parent
        while str(parent) != ".":
            entries.setdefault(str(parent), (stat.S_IFDIR | 0o755, b""))
            parent = parent.parent
    for index, name in enumerate([*sorted(entries), "TRAILER!!!"], 1):
        mode, data = entries.get(name, (0, b""))
        encoded = name.encode() + b"\0"
        fields = [index, mode, 0, 0, 1, 0, len(data), 0, 0, 0, 0, len(encoded), 0]
        result.extend(b"070701" + "".join(f"{value:08x}" for value in fields).encode() + encoded)
        result.extend(b"\0" * (-len(result) % 4))
        result.extend(data)
        result.extend(b"\0" * (-len(result) % 4))
    result.extend(b"\0" * (-len(result) % 512))
    return gzip.compress(bytes(result), compresslevel=9, mtime=0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/person1/kernel-vm")
    parser.add_argument("--kernel", type=Path, default=ROOT / "kernel/build/arch/x86/boot/bzImage")
    parser.add_argument("--qemu-root", type=Path)
    parser.add_argument("--scheduler", type=Path, help="include actual scheduler start/progress/stop/crash tests inside the VM")
    parser.add_argument("--timeout", type=int, default=120)
    parser.add_argument("--accel", choices=["kvm", "tcg"], default="kvm")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    kernel = args.kernel.resolve()
    if not kernel.is_file():
        parser.error("build the pinned kernel first: scripts/build-kernel.sh")
    source_revision = subprocess.check_output(["git", "-C", str(ROOT / "third_party/linux"), "rev-parse", "HEAD"], text=True).strip()
    if source_revision != (ROOT / "kernel/COMMIT").read_text().strip():
        parser.error("kernel source is not at the pinned revision")
    qemu_root = args.qemu_root
    if qemu_root is None and not shutil.which("qemu-system-x86_64"):
        qemu_root = ROOT / "artifacts/person1/qemu-local/root"
    environment = os.environ.copy()
    command = ["qemu-system-x86_64"]
    if qemu_root:
        qemu_root = qemu_root.resolve()
        command = [str(qemu_root / "usr/bin/qemu-system-x86_64"), "-L", str(qemu_root / "usr/share/qemu")]
        environment.update(LD_LIBRARY_PATH=str(qemu_root / "usr/lib"), QEMU_MODULE_DIR=str(qemu_root / "usr/lib/qemu"))
    init = output / "init"
    compile_command = ["gcc", "-static", "-O2", "-Wall", "-Wextra", "-Werror", "-Wl,--build-id=none", "-o", str(init), str(ROOT / "tests/resource/platform_guest.c")]
    subprocess.run(compile_command, check=True)
    first_binary = init.read_bytes()
    subprocess.run(compile_command, check=True)
    if first_binary != init.read_bytes():
        raise RuntimeError("init build is not reproducible")
    files = {"init": (stat.S_IFREG | 0o755, first_binary)}
    for directory in ["dev", "proc", "sys", "tmp", "run", "run/cerynth"]:
        files[directory] = (stat.S_IFDIR | 0o755, b"")
    binary_hashes = {}
    if args.scheduler:
        scheduler = args.scheduler.resolve()
        files["usr/lib/cerynth/cerynth-scx"] = (stat.S_IFREG | 0o755, scheduler.read_bytes())
        binary_hashes["cerynth-scx"] = sha(scheduler)
        # Bundle the actual loader and its runtime dependencies without symlinks
        # into the host. ldd is used only on this repository's trusted binary.
        listing = subprocess.check_output(["ldd", str(scheduler)], text=True)
        if "not found" in listing:
            raise RuntimeError("scheduler has unresolved dynamic libraries")
        for entry in re.findall(r"(?:=>\s+)?(/\S+)\s+\(", listing):
            library = Path(entry)
            files[str(library).lstrip("/")] = (stat.S_IFREG | 0o755, library.read_bytes())
            binary_hashes[str(library)] = sha(library)
        program_headers = subprocess.check_output(["readelf", "-l", str(scheduler)], text=True)
        interpreter = re.search(r"Requesting program interpreter: ([^\]]+)", program_headers)
        if interpreter:
            loader = Path(interpreter.group(1))
            files[str(loader).lstrip("/")] = (stat.S_IFREG | 0o755, loader.read_bytes())
            binary_hashes[str(loader)] = sha(loader)
    first_archive = archive(files)
    if first_archive != archive(dict(reversed(list(files.items())))):
        raise RuntimeError("initramfs depends on filesystem enumeration order")
    initrd = output / "initramfs.cpio.gz"
    initrd.write_bytes(first_archive)
    command += ["-name", "cerynth-platform-validation", "-machine", "q35", "-accel", args.accel,
                "-cpu", "host" if args.accel == "kvm" else "max", "-smp", "2", "-m", "512M",
                "-kernel", str(kernel), "-initrd", str(initrd), "-append", "console=ttyS0 panic=-1 oops=panic rdinit=/init",
                "-no-reboot", "-nographic", "-monitor", "none", "-nic", "none"]
    log = output / "console.log"
    started = time.monotonic()
    timed_out = False
    with log.open("wb") as console:
        process = subprocess.Popen(command, env=environment, stdout=console, stderr=subprocess.STDOUT)
        try:
            returncode = process.wait(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            process.kill()
            returncode = process.wait()
            timed_out = True
    text = log.read_text(errors="replace")
    checks = []
    profiles, faults = [], []
    scope = "resources"
    for line in text.splitlines():
        if line.startswith("CERYNTH_PROFILE "):
            profiles.append(line.split(" ", 1)[1]); scope = "profile/" + profiles[-1]
        elif line.startswith("CERYNTH_FAULT "):
            faults.append(line.split(" ", 1)[1]); scope = "fault/" + faults[-1]
        elif line.startswith("CERYNTH_CHECK "):
            checks.append({**json.loads(line.split(" ", 1)[1]), "scope": scope})
    results = [json.loads(line.split("CERYNTH_RESULT ", 1)[1]) for line in text.splitlines() if line.startswith("CERYNTH_RESULT ")]
    passed = not timed_out and returncode == 0 and bool(results) and results[-1]["failures"] == 0 and bool(checks) and all(check["passed"] for check in checks)
    if args.scheduler:
        passed = passed and profiles == ["balanced", "interactive", "performance", "background"] and len(set(faults)) == 7
    build_manifest = kernel.parents[3] / "cerynth-build.json"
    provenance = json.loads(build_manifest.read_text()) if build_manifest.exists() else None
    if provenance:
        if provenance["source_revision"] != source_revision or provenance["files_sha256"]["arch/x86/boot/bzImage"] != sha(kernel) or provenance["files_sha256"][".config"] != sha(kernel.parents[3] / ".config"):
            raise RuntimeError("kernel image/configuration differ from build provenance")
    report = {"schema_version": 1, "kind": "real-kernel-resource-vm", "passed": passed,
              "acceleration": args.accel, "returncode": returncode, "timeout": timed_out, "duration_seconds": time.monotonic() - started,
              "kernel_source_revision": source_revision, "kernel_sha256": sha(kernel), "kernel_config_sha256": sha(kernel.parents[3] / ".config"),
              "guest_init_sha256": sha(init), "initramfs_sha256": sha(initrd), "double_build_identical": True,
              "scheduler_tested": bool(args.scheduler), "bundled_binary_hashes": binary_hashes,
              "profiles": profiles, "additional_fault_scenarios": faults, "kernel_build_provenance": provenance,
              "compiler": subprocess.check_output(["gcc", "--version"], text=True).splitlines()[0],
              "qemu": subprocess.check_output(command[:1] + ["--version"], env=environment, text=True).splitlines()[0],
              "checks": checks, "console": str(log.relative_to(ROOT)),
              "limitations": ["No desktop/systemd in this kernel test appliance", "I/O weight readback does not prove device fairness",
                              "No arbitrary GPU enforcement", "No sched_ext CPU-weight guarantee"]}
    (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": passed, "checks": len(checks), "report": str(output / "result.json")}))
    if not passed:
        print(text[-6000:])
        raise SystemExit(1)


if __name__ == "__main__":
    main()
