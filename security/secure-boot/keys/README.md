# Signing key management

## Production keys

Production keys MUST be generated and stored outside this repository — in an
HSM, smart card, or offline air-gapped key store controlled by the authorized
key-management process (Person 2). Only public certificates (`*.crt`, `*.cer`,
`*.esl`) may be committed here.

## Test keys

`scripts/generate-test-keys.sh` generates a throwaway PK/KEK/db key hierarchy in
`security/secure-boot/keys/test/`. The output directory is gitignored.

Test keys are for QEMU/OVMF development environments only. Never enroll them
on physical hardware.

## Key hierarchy

| Key | Purpose | Signed by |
|-----|---------|-----------|
| PK  | Platform Key; root of trust for firmware | self-signed |
| KEK | Key Exchange Key; signs db/dbx updates | PK |
| db  | Allowed signature database; signs binaries | KEK |
| dbx | Revoked signature database; signs revocations | KEK |

## Rotation and revocation

1. Generate a new key pair in the authorized key store.
2. Sign the new key with the appropriate parent (KEK for db, PK for KEK).
3. Enroll the new certificate into the corresponding UEFI database.
4. Add the old key's signature to `dbx` to revoke it.
5. Verify that the old key can no longer authorize a boot.
6. Retain the old signed kernel/ bootloader only as a rollback artifact until
   the new chain has been validated.
