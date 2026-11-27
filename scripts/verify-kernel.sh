#!/usr/bin/env bash
# Verify an Authenticode signature on a kernel image using sbverify.
# Without --cert, checks only that a signature exists (not its identity).
set -euo pipefail

usage() {
  echo 'usage: scripts/verify-kernel.sh --image <signed-image> [--cert <db.crt>]' >&2
}

IMAGE=''
CERT=''

while [[ $# -gt 0 ]]; do
  case "$1" in
    --image) IMAGE="${2:-}"; shift 2 ;;
    --cert) CERT="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage; exit 2 ;;
  esac
done

[[ -n "$IMAGE" ]] || { usage; exit 2; }
[[ -f "$IMAGE" ]] || { echo "error: image not found: $IMAGE" >&2; exit 1; }
command -v sbverify >/dev/null || { echo 'error: sbverify is required (install sbsigntool)' >&2; exit 1; }

if [[ -n "$CERT" ]]; then
  [[ -f "$CERT" ]] || { echo "error: certificate not found: $CERT" >&2; exit 1; }
  sbverify --cert "$CERT" "$IMAGE"
else
  sbverify --list "$IMAGE"
fi
echo "Signature verification passed for $IMAGE"
