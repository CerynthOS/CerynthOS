# Boot-chain verification

## Pre-boot verification (offline)

`scripts/verify-kernel.sh` checks that a kernel image carries a valid Authenticode
signature from the expected db certificate. It uses `sbverify` from
`sbsigntool`.

```
scripts/verify-kernel.sh --image <bzImage> --cert <db.crt>
```

This verifies the signature only. It does **not** prove the signing certificate
is enrolled in firmware `db`. That is checked by a successful Secure Boot.

## Firmware verification

`efi-readvar -v db` lists enrolled certificates. Compare the fingerprint of the
expected db certificate against the enrolled entry.

## Runtime verification

After boot, confirm the kernel was loaded through Secure Boot:

```
cat /sys/kernel/security/secureboot     # 1 when enabled
dmesg | grep -i 'secure boot'
```

These are host observations. Recording them in a release report is separate
from performing the verification.

## Boot-chain test matrix (QEMU/OVMF)

1. Signed kernel + enrolled db cert -> boots.
2. Unsigned kernel -> firmware rejects with security violation.
3. Kernel signed by a key not in db -> firmware rejects.
4. Kernel signed by a revoked key -> firmware rejects.
5. Failed update leaves prior kernel bootable.

Each case needs a live OVMF variable store and QEMU. The executed six-case
matrix and its limits are documented in
[nonphysical qualification](../../../docs/platform/nonphysical-qualification.md).
These VM results do not qualify physical firmware or production keys.

Signature listing uses explicit `--list`; certificate verification requires `--cert`. A listing is not reported as trust verification.
