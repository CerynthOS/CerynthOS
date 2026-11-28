#!/usr/bin/env python3
"""Record a completed kernel build; called after make succeeds."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-date-epoch", required=True, type=int)
    parser.add_argument("--build-directory", type=Path, default=ROOT / "kernel/build")
    args = parser.parse_args()
    build = args.build_directory.resolve()
    files = ["arch/x86/boot/bzImage", ".config", "vmlinux"]
    hashes = {}
    for name in files:
        digest = hashlib.sha256()
        with (build / name).open("rb") as source:
            while block := source.read(1024 * 1024): digest.update(block)
        hashes[name] = digest.hexdigest()
    revision = subprocess.check_output(["git", "-C", str(ROOT / "third_party/linux"), "rev-parse", "HEAD"], text=True).strip()
    if revision != (ROOT / "kernel/COMMIT").read_text().strip(): raise SystemExit("kernel source pin changed during build")
    if subprocess.check_output(["git", "-C", str(ROOT / "third_party/linux"), "status", "--porcelain"], text=True).strip(): raise SystemExit("kernel source is dirty")
    report = {"schema_version": 1, "source_revision": revision, "source_date_epoch": args.source_date_epoch,
              "kernel_release": (build / "include/config/kernel.release").read_text().strip(),
              "files_sha256": hashes, "compiler": subprocess.check_output(["gcc", "--version"], text=True).splitlines()[0],
              "double_kernel_build_qualified": False}
    destination = build / "cerynth-build.json"
    destination.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Kernel build provenance: {destination}")


if __name__ == "__main__": main()
