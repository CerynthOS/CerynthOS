#!/usr/bin/env bash
# Certificate verification and signature listing are explicitly different actions.
set -euo pipefail
usage() { echo 'usage: scripts/verify-kernel.sh --image IMAGE (--cert CERT | --list)' >&2; }
IMAGE=''; CERT=''; LIST=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --image|--cert)
      [[ $# -ge 2 && -n "$2" ]] || { usage; exit 2; }
      case "$1" in --image) IMAGE="$2";; --cert) CERT="$2";; esac
      shift 2;;
    --list) LIST=1; shift;;
    -h|--help) usage; exit 0;;
    *) usage; exit 2;;
  esac
done
[[ -n "$IMAGE" && -f "$IMAGE" ]] || { usage; exit 2; }
if [[ "$LIST" -eq 1 ]]; then
  [[ -z "$CERT" ]] || { usage; exit 2; }
else
  [[ -n "$CERT" && -f "$CERT" ]] || { echo 'error: verification requires an explicitly trusted certificate; use --list only for inventory' >&2; exit 2; }
fi
command -v sbverify >/dev/null || { echo 'error: sbverify is required' >&2; exit 1; }
if [[ "$LIST" -eq 1 ]]; then
  sbverify --list "$IMAGE"
  echo 'Signature inventory only; trust was not verified.'
else
  sbverify --cert "$CERT" "$IMAGE"
  echo 'Signature verified against the supplied certificate. Firmware revocation policy is separate.'
fi
