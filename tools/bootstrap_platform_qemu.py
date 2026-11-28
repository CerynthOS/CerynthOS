#!/usr/bin/env python3
"""Extract signature-verified Arch QEMU packages without changing the host.

Uses the existing pacman database and installed Arch trust keyring. No database
refresh, package installation, scriptlet execution, or privilege escalation.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import subprocess
import urllib.parse
import urllib.request
import urllib.error


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, capture_output=True, **kwargs).stdout


def fetch(url, target):
    if not target.exists():
        temporary = target.with_suffix(target.suffix + ".download")
        with urllib.request.urlopen(url, timeout=60) as response, temporary.open("wb") as output:
            while chunk := response.read(1024 * 1024):
                output.write(chunk)
        temporary.replace(target)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("artifacts/person1/qemu-local"))
    parser.add_argument("--extra-package", action="append", default=[], help="additional signature-verified build tools (for example pahole)")
    parser.add_argument("--packages-only", nargs="+", help="extract specific packages instead of the default QEMU set")
    parser.add_argument("--skip-dependency-resolution", action="store_true", help="use an existing tools tree; verify packages but do not resolve host dependency conflicts")
    args = parser.parse_args()
    destination = args.output.resolve()
    packages = destination / "packages"
    root = destination / "root"
    packages.mkdir(parents=True, exist_ok=True)
    root.mkdir(exist_ok=True)
    keyring = Path("/usr/share/pacman/keyrings/archlinux.gpg")
    if not keyring.is_file():
        parser.error("requires an installed Arch Linux trust keyring; use a system QEMU on other distributions")
    trusted_keys = destination / "archlinux.gpg"
    # Arch ships an armored key bundle; gpgv requires a binary keyring.
    run("gpg", "--batch", "--yes", "--dearmor", "--output", str(trusted_keys), str(keyring))
    gpg_home = destination / "gnupg"
    gpg_home.mkdir(mode=0o700, exist_ok=True)
    requested = args.packages_only or ["qemu-system-x86", "qemu-img", *args.extra_package]
    plan = run("pacman", "-Sp", *(["--nodeps", "--nodeps"] if args.skip_dependency_resolution else []), "--print-format", "%n %s %l", *requested)

    def prepare(line):
        name, size, url = line.split(maxsplit=2)
        filename = Path(urllib.parse.unquote(urllib.parse.urlparse(url).path)).name
        archive = packages / filename
        try:
            fetch(url, archive)
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
            url = f"https://archive.archlinux.org/packages/{name[0]}/{name}/{urllib.parse.quote(filename)}"
            fetch(url, archive)
        signature = packages / (filename + ".sig")
        # Cached local packages may not have a sidecar signature. Retrieve the
        # exact signed archive's signature from the official archive mirror.
        signature_url = url + ".sig"
        if url.startswith("file:") and not Path(urllib.parse.urlparse(signature_url).path).exists():
            signature_url = f"https://archive.archlinux.org/packages/{name[0]}/{name}/{urllib.parse.quote(filename)}.sig"
        try:
            fetch(signature_url, signature)
        except urllib.error.HTTPError as error:
            raise RuntimeError(f"signature unavailable: {signature_url}") from error
        verification = run("gpgv", "--homedir", str(gpg_home), "--status-fd=1", "--keyring", str(trusted_keys), str(signature), str(archive))
        fingerprint = next((line.split()[2] for line in verification.splitlines() if line.startswith("[GNUPG:] VALIDSIG ")), None)
        if not fingerprint:
            raise RuntimeError(f"missing valid package signature: {name}")
        members = run("bsdtar", "-tf", str(archive)).splitlines()
        if any(Path(member).is_absolute() or ".." in Path(member).parts for member in members):
            raise RuntimeError(f"unsafe archive path in {name}")
        return archive, {"name": name, "url": url, "bytes": archive.stat().st_size,
                         "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(), "signer": fingerprint}

    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        verified = list(executor.map(prepare, plan.splitlines()))
    # Every package is verified before any extraction occurs.
    for archive, _ in verified:
        run("bsdtar", "--no-same-owner", "-xf", str(archive), "-C", str(root))
    environment = os.environ | {"LD_LIBRARY_PATH": str(root / "usr/lib"), "QEMU_MODULE_DIR": str(root / "usr/lib/qemu")}
    executable = root / "usr/bin/qemu-system-x86_64"
    version = run(str(executable), "--version", env=environment).splitlines()[0] if executable.exists() else None
    previous = json.loads((destination / "manifest.json").read_text()) if (destination / "manifest.json").exists() else {}
    records = {record["name"]: record for record in previous.get("packages", [])}
    records.update({record["name"]: record for _, record in verified})
    manifest = {"version": 1, "qemu": version, "host_modified": False,
                "packages": list(records.values())}
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({"root": str(root), "qemu": version, "packages": len(verified)}))


if __name__ == "__main__":
    main()
