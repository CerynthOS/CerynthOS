#!/usr/bin/env python3
"""Run and record local platform gates; never substitutes for release approval."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


def source_manifest():
    paths = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard"], cwd=ROOT, text=True).splitlines()
    prefixes = ("cli/", "crates/", "services/", "schedulers/", "scripts/", "tools/", "kernel/config/", "packaging/", "tests/resource/")
    selected = [name for name in paths if name.startswith(prefixes) or name in ["Cargo.toml", "Cargo.lock", "kernel/COMMIT", "kernel/VERSION", "third_party/scx.revision", ".github/workflows/platform-kernel.yml"]]
    return {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in sorted(set(selected)) if (ROOT / name).is_file()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kernel", type=Path, default=ROOT / "kernel/build-platform-vm/arch/x86/boot/bzImage")
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/person1/qualification")
    parser.add_argument("--accel", choices=["kvm", "tcg"], default="kvm")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    steps = [
        ("format", ["cargo", "fmt", "--all", "--", "--check"], 120),
        ("rust", ["cargo", "test", "--locked", "--workspace"], 900),
        ("python", ["python3", "-m", "unittest", "discover", "-s", "tools", "-p", "test_*.py"], 180),
        ("baseline", ["python3", "tools/platform_inventory.py"], 60),
        ("build", ["cargo", "build", "--locked", "--release", "-p", "cerynthd", "-p", "cerynthctl", "-p", "cerynth-policy", "-p", "cerynth-scx"], 900),
        ("package", ["python3", "tools/package_platform_runtime.py"], 120),
        ("staged-runtime", ["python3", "tools/platform_runtime_smoke.py"], 60),
        ("kernel-vm", ["python3", "tools/platform_vm_smoke.py", "--kernel", str(args.kernel.resolve()), "--scheduler", "target/release/cerynth-scx", "--output", str(output / "kernel-vm"), "--accel", args.accel, "--timeout", "240"], 270),
    ]
    reports = []
    environment = os.environ | {"CARGO_INCREMENTAL": "0"}
    for name, command, timeout in steps:
        started = time.monotonic()
        log = output / (name + ".log")
        timed_out = False
        with log.open("wb") as stream:
            process = subprocess.Popen(command, cwd=ROOT, env=environment, stdout=stream, stderr=subprocess.STDOUT)
            try: code = process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                process.kill(); code = process.wait(); timed_out = True
        record = {"name": name, "passed": code == 0 and not timed_out, "returncode": code, "timed_out": timed_out,
                  "seconds": time.monotonic() - started, "log_sha256": hashlib.sha256(log.read_bytes()).hexdigest()}
        reports.append(record)
        print(json.dumps(record), flush=True)
    rust = (output / "rust.log").read_text(errors="replace")
    counts = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", rust)
    vm_path = output / "kernel-vm/result.json"
    vm = json.loads(vm_path.read_text()) if vm_path.exists() else None
    vm_verified = bool(vm and vm["passed"] and vm["scheduler_tested"] and vm["kernel_build_provenance"])
    report = {"schema_version": 1, "source_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "qualified_source_sha256": source_manifest(),
              "local_platform_checks_passed": all(step["passed"] for step in reports) and vm_verified,
              "rust_tests_passed": sum(int(passed) for passed, _, _ in counts), "rust_tests_ignored": sum(int(ignored) for _, _, ignored in counts),
              "kernel_check_count": len(vm["checks"]) if vm else 0, "gates": reports,
              "public_alpha_qualified": False,
              "remaining_acceptance": ["This command qualifies the core baseline; see the separate platform extension report for image and advanced-controller evidence",
                  "Physical boot, driver compatibility, installation and power-loss testing on an authorized target",
                  "Authorized release signing identity and human dependency/license review",
                  "Representative workload evaluation before promoting experimental policies",
                  "Cross-team interface and release reviews"]}
    (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({key: report[key] for key in ["local_platform_checks_passed", "rust_tests_passed", "kernel_check_count", "public_alpha_qualified"]}), flush=True)
    if not report["local_platform_checks_passed"]: raise SystemExit(1)


if __name__ == "__main__": main()
