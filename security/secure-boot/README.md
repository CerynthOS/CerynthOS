# Secure Boot chain design

This directory defines CerynthOS's authorized boot-chain signing and enrollment
design. It contains **design and tooling only** — no private keys are stored in
this repository, and no key enrollment into firmware is performed here.

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

## What requires external support

- Enrolling PK/KEK/db/dbx into physical firmware (needs authorized operator).
- Obtaining production signing keys from an HSM/KMS.
- Installing `sbsigntool` (`sbsign`, `sbverify`) on the build host.
- Running QEMU with OVMF Secure Boot enabled to test accept/reject cases.
- dm-verity or similar verified-root design for root filesystem integrity
  (Secure Boot alone does not authenticate userspace files).

No successful enrollment, signing, or Secure Boot validation is claimed by this
design documentation. Running the scripts with real keys and firmware is a
separate, externally supported operation.
