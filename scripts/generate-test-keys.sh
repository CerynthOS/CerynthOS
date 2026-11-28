#!/usr/bin/env bash
# Generate a throwaway PK/KEK/db key hierarchy for QEMU/OVMF Secure Boot testing.
# Private keys are written to security/secure-boot/keys/test/ and are gitignored.
# NEVER enroll these keys on physical hardware.
set -euo pipefail
umask 077

ROOT="$(git rev-parse --show-toplevel)"
KEYDIR="$ROOT/security/secure-boot/keys/test"
BITS="${BITS:-2048}"
DAYS="${DAYS:-3650}"

command -v openssl >/dev/null || { echo 'error: openssl is required' >&2; exit 1; }

if [[ -d "$KEYDIR" ]] && compgen -G "$KEYDIR/*.key" >/dev/null; then
  echo "Test keys already exist in $KEYDIR; refusing to overwrite." >&2
  exit 1
fi

mkdir -p "$KEYDIR"
chmod 700 "$KEYDIR"

gen_key() {
  local name="$1" subject="$2"
  openssl req -x509 -newkey "rsa:$BITS" -nodes \
    -keyout "$KEYDIR/$name.key" -out "$KEYDIR/$name.crt" \
    -days "$DAYS" -sha256 -subj "$subject" 2>/dev/null
  chmod 600 "$KEYDIR/$name.key"
  chmod 644 "$KEYDIR/$name.crt"
}

gen_key PK  '/CN=CerynthOS Test PK/O=CerynthOS Test Only'
gen_key KEK '/CN=CerynthOS Test KEK/O=CerynthOS Test Only'
gen_key db  '/CN=CerynthOS Test db/O=CerynthOS Test Only'

cat > "$KEYDIR/README" <<'EOF'
THROWAWAY TEST KEYS — FOR QEMU/OVMF ONLY.
These keys are gitignored. Never enroll them on physical hardware.
EOF

echo "Test keys generated in $KEYDIR"
echo "These are for QEMU/OVMF only. Never enroll on physical hardware."
