# Kernel update and rollback design

## Requirements

1. Build the new kernel from a pinned source revision.
2. Sign the kernel with the authorized db key.
3. Verify the signature before installation.
4. Install without destroying the previous working kernel.
5. Update the bootloader entry to point at the new kernel.
6. Boot and verify the new version.
7. Retain a rollback path to the previous kernel.

## Installation layout

```
/boot/efi/EFI/CerynthOS/
  kernel-<version>.signed    # current signed kernel
  kernel-<previous>.signed   # previous working kernel (retained)
  bootloader.efi             # signed bootloader
```

The bootloader entry is updated atomically. The previous kernel is never
removed during an update; it is pruned only after the new version has been
verified over a defined number of boots.

## Update procedure

1. `scripts/build-kernel.sh --image-only` builds the pinned kernel.
2. `scripts/verify-kernel.sh` confirms the built image's current signature
   state (unsigned by default — this is expected before signing).
3. `scripts/sign-kernel.sh --image <new> --out <new>.signed` signs it.
4. `scripts/verify-kernel.sh --image <new>.signed --cert <db.crt>` verifies it.
5. Copy the signed kernel to the EFI System Partition under a new versioned
   name. Do not overwrite the previous kernel.
6. Update the bootloader entry atomically (write temp config, then rename).
7. Reboot into the new entry.
8. Verify Secure Boot state and kernel version.
9. If verification fails, select the previous bootloader entry to roll back.

## Failure handling

- A signature verification failure aborts the update before touching the
  bootloader configuration.
- A failed boot does not destroy the previous kernel entry.
- Rollback is a bootloader entry selection, not a rebuild.

## What is not automated here

- Actual EFI System Partition writes (require root and host authorization).
- Bootloader configuration edits specific to a deployed boot setup.
- Physical console verification of boot success.

These require an authorized operator on the target machine.
