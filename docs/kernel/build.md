# Building and validating the platform

Use Linux x86-64 with GCC, make, bc, flex, bison, OpenSSL/libelf development
headers, pkg-config, pahole, Python, Rust and Clang. VM tests additionally
need QEMU and a static C library for the guest init program. The tested Arch
toolchain uses GCC 16.2.1, pahole 1.32 and QEMU 11.1.3. This is one supported
test environment, not a complete hardware compatibility claim.

Build scripts verify dependency revisions and refuse dirty upstream source
trees. Preserve local upstream changes before changing revisions. The main
kernel configuration/build directory remains `kernel/build`:

```bash
scripts/configure-kernel.sh
JOBS=4 scripts/build-kernel.sh --image-only
# Omit --image-only when the module build is also needed.
```

For isolated kernel/resource experiments use the small VM profile. This
profile is a test appliance and does not supply desktop drivers, an
installer or a distribution userspace:

```bash
JOBS=4 scripts/vm/build-platform-kernel.sh
scripts/build-scx.sh
python3 tools/platform_vm_smoke.py \
  --kernel kernel/build-platform-vm/arch/x86/boot/bzImage \
  --scheduler target/release/cerynth-scx --timeout 180
```

`kernel/config/platform-vm.fragment` is merged with tinyconfig. Use
`--reconfigure` deliberately after changing the fragment. Override
`CERYNTH_KERNEL_BUILD` to isolate another output directory. Source checkouts
and the workstation's boot configuration remain untouched. Build provenance
is written to `cerynth-build.json` in the output directory, including source,
configuration and binary hashes. A single build manifest does not prove
bit-for-bit kernel reproducibility across machines.

On Arch, missing QEMU/build tools can be extracted into the workspace after
verification against the installed Arch keyring:

```bash
python3 tools/bootstrap_platform_qemu.py --extra-package pahole --extra-package bc
export PATH="$PWD/artifacts/person1/qemu-local/root/usr/bin:$PATH"
export LD_LIBRARY_PATH="$PWD/artifacts/person1/qemu-local/root/usr/lib"
```

This uses the installed pacman database, verifies detached package signatures
before extraction, and runs no package install scripts. It does not refresh
the database or install host packages. Other distributions can use their
normal trusted QEMU/toolchain installation. The package manifest records
versions, SHA-256 digests and signer fingerprints.

VM checks cover kernel boot, resource limits and accounting, bounded OOM,
process-count denial, pressure files, cleanup and scheduler fault recovery.
The host scheduler and cgroups are never changed by this test. A timeout,
failed check, missing completion marker or mismatched build manifest fails
the run. Raw console output is retained alongside the result. `--accel tcg`
allows software emulation; the default requires working KVM access.

For the platform userspace:

```bash
cargo test --locked --workspace
python3 -m unittest discover -s tools -p 'test_*.py'
cargo build --locked --release -p cerynthd -p cerynthctl -p cerynth-policy -p cerynth-scx
python3 tools/package_platform_runtime.py
python3 tools/platform_runtime_smoke.py
python3 tools/platform_inventory.py
```

The package tool produces a deterministic development runtime archive, file
manifest and Cargo dependency/license inventory. It verifies upgrade config
preservation and the existing installation paths without enabling services.
The staged runtime test uses a mock scheduler to exercise actual daemon/CLI
restart and corrupt-state recovery; the VM test supplies real kernel evidence.
Neither result substitutes for physical hardware, installation/upgrade,
release-signing or cross-team acceptance.
