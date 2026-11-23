#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
LINUX="$ROOT/third_party/linux"
BUILD="$(realpath -m "${CERYNTH_KERNEL_BUILD:-$ROOT/kernel/build}")"
JOBS="${JOBS:-$(nproc)}"
# shellcheck source=scripts/lib/source-pins.sh
source "$ROOT/scripts/lib/source-pins.sh"
require_pinned_source "$LINUX" "$ROOT/kernel/COMMIT"
[[ "$JOBS" =~ ^[1-9][0-9]*$ ]] || { echo 'error: JOBS must be positive' >&2; exit 1; }
for tool in gcc make bc flex bison pahole python3; do
  command -v "$tool" >/dev/null || { printf 'error: missing kernel build tool: %s\n' "$tool" >&2; exit 1; }
done
IMAGE_ONLY=0
if [[ "${1:-}" == "--image-only" && "$#" == 1 ]]; then
  IMAGE_ONLY=1
elif [[ "$#" != 0 ]]; then
  echo 'usage: scripts/build-kernel.sh [--image-only]' >&2; exit 2
fi

if [[ ! -f "$BUILD/.config" ]]; then
  "$ROOT/scripts/configure-kernel.sh"
fi

export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$LINUX" show -s --format=%ct HEAD)}"
KBUILD_BUILD_TIMESTAMP="$(date -u -d "@$SOURCE_DATE_EPOCH" '+%a %b %d %T UTC %Y')"
export KBUILD_BUILD_TIMESTAMP
export KBUILD_BUILD_USER=cerynth KBUILD_BUILD_HOST=build KBUILD_BUILD_VERSION=1
export LC_ALL=C
if [[ "$IMAGE_ONLY" == 1 ]]; then
  make -C "$LINUX" O="$BUILD" -j"$JOBS" KCFLAGS="-Werror=implicit-function-declaration" bzImage
else
  make -C "$LINUX" O="$BUILD" -j"$JOBS" KCFLAGS="-Werror=implicit-function-declaration"
  make -C "$LINUX" O="$BUILD" -j"$JOBS" modules
fi

python3 "$ROOT/tools/kernel_build_manifest.py" --source-date-epoch "$SOURCE_DATE_EPOCH" --build-directory "$BUILD"

echo
echo "Kernel image:"
ls -lh "$BUILD/arch/x86/boot/bzImage"
