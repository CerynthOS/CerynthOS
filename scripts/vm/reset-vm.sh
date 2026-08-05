#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

BASE="$ROOT_DIR/vm/images/ubuntu-noble-base.img"
OVERLAY="$ROOT_DIR/vm/overlays/cerynth-dev.qcow2"

[[ -f "$BASE" ]] || {
    echo "Base image missing: $BASE" >&2
    exit 1
}

if pgrep -af qemu-system-x86_64 | grep -q "$OVERLAY"; then
    echo "Refusing to reset: VM appears to be running." >&2
    exit 1
fi

printf 'This will erase every change made inside the CerynthOS VM.\n'
read -r -p 'Type RESET to continue: ' confirmation

[[ "$confirmation" == "RESET" ]] || {
    echo "Reset cancelled."
    exit 0
}

rm -f "$OVERLAY"

qemu-img create \
  -f qcow2 \
  -F qcow2 \
  -b "$BASE" \
  "$OVERLAY"

echo "VM overlay reset successfully."
