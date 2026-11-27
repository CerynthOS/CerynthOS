#!/usr/bin/env bash
# Sign a kernel image with an authorized db key using sbsign (Authenticode).
# Private keys are never logged or embedded in the output.
set -euo pipefail

usage() {
  echo 'usage: scripts/sign-kernel.sh --image <unsigned-image> --out <signed-image> [--key <db.key>] [--cert <db.crt>]' >&2
}

IMAGE=''
OUT=''
KEY="${CERYNTH_DB_KEY:-}"
CERT="${CERYNTH_DB_CERT:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --image) IMAGE="${2:-}"; shift 2 ;;
    --out) OUT="${2:-}"; shift 2 ;;
    --key) KEY="${2:-}"; shift 2 ;;
    --cert) CERT="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage; exit 2 ;;
  esac
done

[[ -n "$IMAGE" && -n "$OUT" ]] || { usage; exit 2; }
[[ -f "$IMAGE" ]] || { echo "error: image not found: $IMAGE" >&2; exit 1; }
[[ -n "$KEY" && -n "$CERT" ]] || { echo 'error: provide --key/--cert or CERYNTH_DB_KEY/CERYNTH_DB_CERT' >&2; exit 1; }
[[ -f "$KEY" && -f "$CERT" ]] || { echo 'error: key or certificate file not found' >&2; exit 1; }
command -v sbsign >/dev/null || { echo 'error: sbsign is required (install sbsigntool)' >&2; exit 1; }

# Refuse to sign a file that appears to be a private key by mistake.
if [[ "$IMAGE" == "$KEY" ]]; then
  echo 'error: image path equals key path' >&2
  exit 1
fi

mkdir -p "$(dirname "$OUT")"
sbsign --key "$KEY" --cert "$CERT" --output "$OUT" "$IMAGE"
echo "Signed kernel written to $OUT (signature not verified here; run verify-kernel.sh)."
