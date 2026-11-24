# Development images and recovery

These are x86-64 development artifacts. They do not assert physical hardware
qualification, Secure Boot, a trusted public release identity, or team approval.
The kernel test appliance and full distribution images serve different purposes.

## Build

`packaging/image` uses mkosi 27.1 and the Arch snapshot dated 2026/11/20.
Package signatures remain mandatory. Prefetching uses the snapshot's exact
filenames and SHA256 values; a current mirror can supply identical package
bytes, while final installation still authenticates packages with pacman.
`--offline` refuses network package downloads and needs a populated cache.
The workspace-local tools avoid installing packages into the development host.

```sh
python3 tools/bootstrap_platform_qemu.py --extra-package mkosi --extra-package systemd-ukify --extra-package python-pefile --extra-package dosfstools --extra-package mtools
python3 tools/prepare_os_image.py --development-access
python3 tools/build_os_image.py --tool-root artifacts/person1/qemu-local/root
```

The first build syncs the pinned metadata. `tools/prefetch_image_packages.py`
can fill the cache concurrently once those databases exist. Subsequent builds
may use `--offline`. The builder propagates a fixed source epoch and partition
seed, but byte-identical full-image builds are not asserted: filesystem UUIDs,
filesystem population and external package tools still need comparison.

The base variant writes `artifacts/person1/os-image`. For KDE Plasma/KWin,
Konsole, Dolphin, SDDM and the existing native observer:

```sh
python3 tools/prepare_os_image.py --development-access --desktop
python3 tools/build_os_image.py --tool-root artifacts/person1/qemu-local/root --desktop
```

That variant writes `artifacts/person1/os-image-desktop`. Development access
creates one dedicated SSH key under the private ignored `image-access`
directory; only its public half enters the image. It also enables local desktop
autologin for `cerynth` in the desktop variant. Password SSH authentication is
disabled. For an image without that convenience, omit `--development-access`;
an existing public key can be supplied with `--ssh-public-key`. Private release
keys and VM access keys are never copied into image payloads or Git.

The distribution kernel is 7.3.5-arch1-1. The frozen SCX loader was qualified on
the separate pinned 7.1 test kernel, so SCX autostart and adaptation remain off
in the distribution image. Ordinary Linux scheduling, wired DHCP, core services
and the user runtime work without a model server or cloud account.

## Boot qualification

```sh
python3 tools/os_image_vm.py --tool-root artifacts/person1/qemu-local/root
python3 tools/os_image_vm.py --tool-root artifacts/person1/qemu-local/root --desktop --image artifacts/person1/os-image-desktop/cerynthos.raw --output artifacts/person1/os-image-desktop-boot.json
```

The runner boots OVMF/UEFI through the actual disk's bootloader into an isolated
qcow2 overlay. It checks services and enforced swap, cpuset and direct-I/O limits,
then cuts VM power and verifies a persisted marker after reboot. External guest
network access is disabled. The desktop check requires KWin Wayland, Plasma and
Observer, measures KWin D-Bus replies under managed CPU load, and saves a QMP
screenshot. It measures VM service responsiveness, not input-to-photon latency.

## A/B updates

The `--ab` build profile creates a 1 GiB EFI partition, two 6 GiB root slots,
and separate 1 GiB `/var` and `/home` partitions. These capacities suit the
development tests; production sizing is a separate installation decision.
The root slots have explicit, fixed partition identities. Never attach cloned
disks with these same identities to one running machine.

```sh
python3 tools/prepare_os_image.py --development-access
python3 tools/build_os_image.py --tool-root artifacts/person1/qemu-local/root --ab
python3 tools/package_ab_update.py --development-key --sequence 2 --version 0.1.0-dev-trial --output artifacts/person1/update-trial
python3 tools/ab_update_smoke.py --tool-root artifacts/person1/qemu-local/root --bundle artifacts/person1/update-trial
```

The controller verifies the externally trusted signature and compressed payload
hashes before writing. It identifies the running root and refuses active or
mounted targets, wrong-disk slots, size mismatches and sequence replay. It writes
and reads back only the inactive root, preserves the explicit configuration
allowlist, installs the matching UKI, flushes EFI files, and selects a one-shot
trial entry. No reboot occurs during staging. The retained default remains the
previous known-good slot until the health service accepts the trial.

The health service checks the core control API and user services/API. A healthy
trial becomes the default; a failed trial retains the previous default. The
test also exercises an unbootable trial and manual rollback across subsequent
reboots. `/var` and `/home` persist separately. The update preserves
`/etc/cerynth`, machine identity and the Ed25519 SSH host key. Arbitrary `/etc`
migrations are not guessed. This is a development migration policy, not a promise
that all distribution package/configuration upgrades are automatically safe.

Inside the authorized installed A/B OS, its administrator runs:

```sh
/usr/lib/cerynth/cerynth-slot-update stage --directory /absolute/update --public-key /absolute/trusted-public-key.pem
/usr/lib/cerynth/cerynth-slot-update status
/usr/lib/cerynth/cerynth-slot-update rollback
```

Stage and rollback report when a reboot is required. Sequence numbers are
monotonic even after a failed attempt; publish a new signed sequence for a retry.
A power cut during installation of a fresh disk is different from an A/B update:
the fresh target must be reinstalled before booting it.

## Artifact signing, installation and inventory

`tools/release_os_image.py` signs manifests with an explicitly selected Ed25519
key. Verification uses an external trusted public key, never a key selected by
the bundle. `install` authenticates all artifacts before opening the target,
uses an installation journal and verifies installed bytes. New regular-file
targets support disposable tests. A whole block device additionally requires
root, `--erase-all-data`, its exact `--expected-serial`, sufficient capacity and
no mounted child partitions. No host block-device installation is performed by
the qualification tools.

```sh
python3 tools/release_image_smoke.py
python3 tools/image_sbom.py --manifest artifacts/person1/os-image-desktop/cerynthos.manifest --output artifacts/person1/desktop-sbom.cdx.json
```

The inventory retains declared package licenses and the Cargo build graph,
including build/dev dependencies. It does not conclude binary reachability or
grant release approval. GPL headers in the reused SCX/BPF sources remain intact;
the workspace's declared license is accompanied by its Apache-2.0 text. A human
dependency/license review and authorized release identity remain release gates.

Troubleshooting: preserve the build/serial logs; inspect the JSON status and
system/user journals. Missing FUSE helpers, cpuset/I/O delegation, PE-parser or
FAT population tools are actionable failures. Do not enable SCX as a workaround
for a resource controller failure. Keep the previous boot slot and known-good
kernel available while diagnosing an update.
