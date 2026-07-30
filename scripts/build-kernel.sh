#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
LINUX="$ROOT/third_party/linux"
BUILD="$ROOT/kernel/build"
JOBS="${JOBS:-$(nproc)}"

if [[ ! -f "$BUILD/.config" ]]; then
  "$ROOT/scripts/configure-kernel.sh"
fi

make -C "$LINUX" O="$BUILD" -j"$JOBS" \
  KCFLAGS="-Werror=implicit-function-declaration"

make -C "$LINUX" O="$BUILD" -j"$JOBS" modules

echo
echo "Kernel image:"
ls -lh "$BUILD/arch/x86/boot/bzImage"
