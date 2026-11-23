#!/usr/bin/env python3
"""Stage and reproducibly package the existing platform runtime contract.

Produces a development payload, file hashes and a Cargo dependency inventory.
Does not install, enable services, sign releases or claim a bootable OS image.
"""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def sha(data): return hashlib.sha256(data).hexdigest()


def payload(stage, epoch):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w", format=tarfile.PAX_FORMAT) as tar:
        for file in sorted(stage.rglob("*")):
            if file.is_symlink(): raise RuntimeError("unexpected symlink in staged runtime")
            info = tar.gettarinfo(str(file), arcname=str(file.relative_to(stage)))
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            info.mtime = epoch
            info.pax_headers = {}
            if file.is_file():
                with file.open("rb") as content: tar.addfile(info, content)
            else: tar.addfile(info)
    return gzip.compress(output.getvalue(), compresslevel=9, mtime=0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/person1/runtime-package")
    args = parser.parse_args()
    destination = args.output.resolve()
    destination.mkdir(parents=True, exist_ok=True)
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", subprocess.check_output(["git", "show", "-s", "--format=%ct", "HEAD"], cwd=ROOT, text=True).strip()))
    with tempfile.TemporaryDirectory(prefix="cerynth-platform-package-") as directory:
        stage = Path(directory)
        subprocess.run(["bash", str(ROOT / "scripts/install-dev-runtime.sh"), "--root", str(stage), "--no-enable"], cwd=ROOT, check=True, capture_output=True)
        subprocess.run(["bash", str(ROOT / "scripts/check-runtime-files.sh"), str(stage)], check=True, capture_output=True)
        manifest = {str(file.relative_to(stage)): {"sha256": sha(file.read_bytes()), "mode": oct(file.stat().st_mode & 0o777), "bytes": file.stat().st_size} for file in sorted(stage.rglob("*")) if file.is_file()}
        first = payload(stage, epoch)
        if first != payload(stage, epoch): raise RuntimeError("runtime archive is not reproducible")
        # Upgrade staging must preserve a user's configuration and ship .dist.
        config = stage / "etc/cerynth/cerynth.toml"
        config.write_text("# preserve owner configuration\n")
        subprocess.run(["bash", str(ROOT / "scripts/install-dev-runtime.sh"), "--root", str(stage), "--no-enable"], cwd=ROOT, check=True, capture_output=True)
        if config.read_text() != "# preserve owner configuration\n": raise RuntimeError("upgrade overwrote configuration")
        if (config.parent / "cerynth.toml.dist").read_bytes() != (ROOT / "packaging/config/cerynth.toml").read_bytes(): raise RuntimeError("default upgrade configuration missing")
    archive = destination / "cerynth-platform-runtime.tar.gz"
    archive.write_bytes(first)
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--offline", "--format-version", "1"], cwd=ROOT, text=True))
    dependencies = sorted([{"name": package["name"], "version": package["version"], "license": package["license"], "source": package["source"] or "workspace", "repository": package["repository"]} for package in metadata["packages"]], key=lambda item: (item["name"], item["version"]))
    (destination / "dependency-inventory.json").write_text(json.dumps({"schema_version": 1, "format": "cerynth-cargo-inventory", "packages": dependencies, "license_review_complete": False}, indent=2) + "\n")
    report = {"schema_version": 1, "kind": "reproducible-platform-development-payload", "source_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "tracked_source_dirty": bool(subprocess.check_output(["git", "diff", "--name-only", "HEAD"], cwd=ROOT, text=True).strip()),
              "source_date_epoch": epoch, "archive_sha256": sha(first), "cargo_lock_sha256": sha((ROOT / "Cargo.lock").read_bytes()),
              "payload": manifest, "double_archive_identical": True, "upgrade_preserves_configuration": True,
              "host_installation_modified": False, "bootable_distribution": False, "signed_release": False,
              "limitations": ["Requires compatible Linux userspace libraries", "Dependency inventory does not constitute a license or security audit"]}
    (destination / "manifest.json").write_text(json.dumps(report, indent=2) + "\n")
    (destination / "SHA256SUMS").write_text(f"{sha(first)}  {archive.name}\n")
    print(json.dumps({"archive": str(archive), "sha256": sha(first), "files": len(manifest), "cargo_packages": len(dependencies)}))


if __name__ == "__main__": main()
