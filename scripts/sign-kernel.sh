#!/usr/bin/env bash
# Create a separate Authenticode-signed PE image; never overwrite key material.
set -euo pipefail
umask 077
usage() { echo 'usage: scripts/sign-kernel.sh --image IMAGE --out NEW_IMAGE --key KEY --cert CERT' >&2; }
IMAGE=''; OUT=''; KEY="${CERYNTH_DB_KEY:-}"; CERT="${CERYNTH_DB_CERT:-}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --image|--out|--key|--cert)
      [[ $# -ge 2 && -n "$2" ]] || { usage; exit 2; }
      case "$1" in --image) IMAGE="$2";; --out) OUT="$2";; --key) KEY="$2";; --cert) CERT="$2";; esac
      shift 2;;
    -h|--help) usage; exit 0;;
    *) usage; exit 2;;
  esac
done
[[ -n "$IMAGE" && -n "$OUT" && -n "$KEY" && -n "$CERT" ]] || { usage; exit 2; }
[[ -f "$IMAGE" && -f "$KEY" && -f "$CERT" ]] || { echo 'error: image, key or certificate missing' >&2; exit 1; }
[[ ! "$IMAGE" -ef "$KEY" && ! "$IMAGE" -ef "$CERT" ]] || { echo 'error: image aliases key material' >&2; exit 1; }
[[ ! -e "$OUT" && ! -L "$OUT" ]] || { echo 'error: output must be a new file' >&2; exit 1; }
for protected in "$IMAGE" "$KEY" "$CERT"; do
  [[ "$(realpath -m -- "$OUT")" != "$(realpath -- "$protected")" ]] || { echo 'error: output aliases protected input' >&2; exit 1; }
done
command -v sbsign >/dev/null || { echo 'error: install sbsigntools or add its workspace-local bin directory to PATH' >&2; exit 1; }
command -v sbverify >/dev/null || { echo 'error: sbverify is required to verify the result before publication' >&2; exit 1; }
mkdir -p -- "$(dirname -- "$OUT")"
TEMP="$(mktemp -- "${OUT}.tmp.XXXXXXXX")"
trap 'rm -f -- "$TEMP"' EXIT
sbsign --key "$KEY" --cert "$CERT" --output "$TEMP" "$IMAGE"
sbverify --cert "$CERT" "$TEMP"
# A no-clobber link publishes the already-verified bytes atomically.
ln -- "$TEMP" "$OUT"
echo 'Signed image created and certificate-verified.'
