# Signing pipeline

## Signed artifacts

- Bootloader (systemd-boot `systemd-bootx64.efi` or GRUB `grubx64.efi`)
- Kernel (`bzImage`, converted to PE format with `objcopy` when needed)
- UEFI applications that run before the kernel

## Signing flow

```
build kernel -> convert to PE -> sbsign with db key -> verify -> install
```

`scripts/sign-kernel.sh` implements the kernel step. It requires:

- `sbsign` from `sbsigntool`
- a db private key (`*.key`) and certificate (`*.crt`)
- an unsigned kernel image (PE format)

Private keys are read from the path supplied on the command line or via
`CERYNTH_DB_KEY` / `CERYNTH_DB_CERT` environment variables. The script never
embeds key material and never writes secrets to logs.

## Feed the signature database

A signed binary alone is not sufficient — its signing certificate must be in
the firmware `db` database. The enrollment procedure in
`security/secure-boot/enrollment/` describes how to build and enroll the EFI
Signature List (ESL) containing the db certificate.

## Bootloader signing

The bootloader must be signed with the same db key (or another certificate in
`db`). Replace the kernel image argument to `scripts/sign-kernel.sh` with the
bootloader EFI binary. The generic `sbsign` invocation is the same.

## Test-key signing

Use `scripts/generate-test-keys.sh` to produce a throwaway hierarchy, then
pass the generated db key/cert to `scripts/sign-kernel.sh`. These signatures
are only accepted by firmware enrolled with the corresponding test keys.

## Verified workspace-local execution

Use `python3 tools/bootstrap_secure_boot_tools.py` and prepend its
`artifacts/person1/qemu-local/root/usr/bin` directory to PATH when invoking the
shell wrappers. Signing requires a new output path, refuses protected-input
aliases, verifies against the selected certificate, then publishes atomically.
The [qualification report](../../../docs/platform/nonphysical-qualification.md)
records actual disposable-key signing, verification and OVMF decisions. No
production key is selected by these development commands.
