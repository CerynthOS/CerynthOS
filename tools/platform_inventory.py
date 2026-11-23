#!/usr/bin/env python3
"""Read-only platform inventory. Availability is not an enforcement claim."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


def read(path):
    try:
        return Path(path).read_text().strip()
    except OSError:
        return None


def revision(directory):
    try:
        return subprocess.check_output(["git", "-C", str(directory), "rev-parse", "HEAD"], text=True, stderr=subprocess.DEVNULL, timeout=5).strip()
    except (OSError, subprocess.SubprocessError):
        return None


def inventory():
    memory = {}
    for line in (read("/proc/meminfo") or "").splitlines():
        key, value = line.split(":", 1)
        fields = value.split()
        memory[key] = int(fields[0]) * (1024 if len(fields) > 1 and fields[1] == "kB" else 1)
    cgroup = Path("/sys/fs/cgroup")
    state = read("/sys/kernel/sched_ext/state")
    controls = {name: (cgroup / name).exists() for name in ["cpu.weight", "cpu.max", "memory.high", "memory.max", "memory.swap.max", "pids.max", "io.weight", "io.max", "cpuset.cpus.effective"]}
    model = next((line.split(":", 1)[1].strip() for line in (read("/proc/cpuinfo") or "").splitlines() if line.startswith("model name")), None)
    kernel_source = revision(ROOT / "third_party/linux")
    scx_source = revision(ROOT / "third_party/scx")
    return {"schema_version": 1, "observed_at_ms": time.time_ns() // 1_000_000,
            "machine": platform.machine(), "kernel": platform.release(), "uid": os.getuid(),
            "cpu": {"model": model, "logical": os.cpu_count(), "allowed": sorted(os.sched_getaffinity(0))},
            "memory_bytes": {key: memory.get(key) for key in ["MemTotal", "MemAvailable", "SwapTotal", "SwapFree"]},
            "pressure": {kind: read(f"/proc/pressure/{kind}") for kind in ["cpu", "memory", "io"]},
            "cgroups": {"version": 2 if (cgroup / "cgroup.controllers").exists() else None,
                        "controllers": (read(cgroup / "cgroup.controllers") or "").split(),
                        "root_interface_availability": controls, "self_membership": read("/proc/self/cgroup"),
                        "authority": "systemd-managed session hierarchy; root paths are inventory only"},
            "scheduler": {"sched_ext_state": state, "cpu_weight_under_sched_ext_qualified": False,
                          "baseline_manifest": "docs/contracts/platform-baseline-v1.json"},
            "source": {"linux": kernel_source, "linux_pin_matches": kernel_source == read(ROOT / "kernel/COMMIT"),
                       "scx": scx_source, "scx_pin_matches": scx_source == read(ROOT / "schedulers/SCX_COMMIT") == read(ROOT / "third_party/scx.revision")},
            "build": {name: shutil.which(name) for name in ["cargo", "rustc", "clang", "gcc", "make", "qemu-system-x86_64", "qemu-img", "systemd-repart", "shellcheck", "gpgv"]},
            "kvm_accessible": os.access("/dev/kvm", os.R_OK | os.W_OK),
            "workspace_free_bytes": shutil.disk_usage(ROOT).free,
            "limitations": ["Read-only observations do not prove delegated controller access", "I/O fairness depends on block device and scheduler", "Physical desktop latency and boot recovery require separate evidence"]}


def check_baseline(path):
    baseline = json.loads(path.read_text())
    failures = []
    for name, digest in baseline["scheduler_files"].items():
        file = ROOT / name
        if not file.is_file() or hashlib.sha256(file.read_bytes()).hexdigest() != digest:
            failures.append(name)
    for name, expected in baseline["dependency_pins"].items():
        if read(ROOT / name) != expected:
            failures.append(name)
    return {"passed": not failures, "differences": failures, "frozen_source_revision": baseline["source_revision"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/person1/platform-inventory.json")
    parser.add_argument("--baseline", type=Path, default=ROOT / "docs/contracts/platform-baseline-v1.json")
    args = parser.parse_args()
    report = inventory()
    report["baseline_integrity"] = check_baseline(args.baseline)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"report": str(args.output), "baseline": report["baseline_integrity"], "kvm": report["kvm_accessible"]}))
    if not report["baseline_integrity"]["passed"] or not report["source"]["linux_pin_matches"] or not report["source"]["scx_pin_matches"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
