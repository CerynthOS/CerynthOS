# Secure Boot chain design

This directory defines CerynthOS's authorized boot-chain signing and enrollment
design. It contains **design, tooling and disposable-VM qualification** — no private keys are stored in
this repository, and no key enrollment into physical firmware is performed here.

## Trust chain

```
UEFI firmware
  -> Platform Key (PK)
    -> Key Exchange Key (KEK)
      -> Allowed signature database (db)
        -> Signed bootloader (systemd-boot / GRUB)
          -> Signed kernel
            -> Initramfs / root filesystem
              -> CerynthOS
```

The revocation database (`dbx`) blocks known-bad signatures and must be kept
current alongside `db`.

## Separation of duties

- **Person 1 (OS)**: owns signing tooling, kernel/build integration, update
  rollback safety, and boot-chain verification.
- **Person 2 (policy/security)**: owns authorization decisions for which keys
  may be enrolled, rotation/revocation policy, and recovery procedures.
- **Production keys**: must be generated and stored in an external HSM or
  offline key-management system. Never commit private keys to this repository.
- **Test keys**: may be generated locally for QEMU/OVMF testing. They are
  gitignored and must never be enrolled on physical hardware.

## Directory layout

```
security/secure-boot/
  keys/          # test-key generation (private keys gitignored)
  signing/       # design notes for the signing pipeline
  enrollment/    # PK/KEK/db/dbx enrollment procedure and templates
  verification/  # boot-chain verification procedure
  update/        # kernel update + rollback design
```

## Scripts (repository root)

- `scripts/build-kernel.sh` — builds the pinned kernel (existing).
- `scripts/sign-kernel.sh` — signs a kernel with `sbsign`.
- `scripts/verify-kernel.sh` — verifies a kernel signature with `sbverify`.
- `scripts/generate-test-keys.sh` — generates throwaway test keys for QEMU.

## What is implemented here

- Test-key generation using OpenSSL (gitignored output).
- Kernel signing and verification scripts with strict dependency checks.
- Detailed enrollment, update, and verification procedures.
- Idempotent update design that preserves the previous working kernel.

## Executed nonphysical qualification

The [November 27 report](../../docs/platform/nonphysical-qualification.md) records
real signed/unsigned/unauthorized/tampered/revoked/fallback OVMF cases, independent
certificate verification and a signed development image with dm-verity corruption
rejection. The runners use private disposable variable stores and test keys.

- `tools/bootstrap_secure_boot_tools.py` provisions private build/verification tools.
- `tools/secure_boot_vm.py` executes the firmware matrix.
- `tools/verified_root_vm.py` boots and corrupts only a disposable verified-root overlay.

## Remaining external acceptance

Physical PK/KEK/db/dbx enrollment still needs the authorized operator. Production
signing identities, HSM/KMS access, trust distribution, license and release decisions
remain external. A test-key VM success does not supply those decisions. The optional
verified-root profile has ephemeral writes and does not qualify persistent A/B
verified-root upgrades. Existing default images are not retroactively Secure Boot
qualified by this report.
