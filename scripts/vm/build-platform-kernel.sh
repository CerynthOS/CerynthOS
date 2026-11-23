#!/usr/bin/env bash
# Build a small, isolated test kernel without replacing the workstation build.
set -Eeuo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BUILD="$(realpath -m "${CERYNTH_KERNEL_BUILD:-$ROOT/kernel/build-platform-vm}")"
LINUX="$ROOT/third_party/linux"
FRAGMENT="$ROOT/kernel/config/platform-vm.fragment"
# shellcheck source=scripts/lib/source-pins.sh
source "$ROOT/scripts/lib/source-pins.sh"
require_pinned_source "$LINUX" "$ROOT/kernel/COMMIT"
[[ "$#" == 0 || ("$#" == 1 && "$1" == --reconfigure) ]] || {
    echo 'usage: scripts/vm/build-platform-kernel.sh [--reconfigure]' >&2; exit 2;
}
for tool in gcc make bc flex bison pahole; do
    command -v "$tool" >/dev/null || { printf 'error: missing build tool: %s\n' "$tool" >&2; exit 1; }
done
mkdir -p "$BUILD"
if [[ "${1:-}" == --reconfigure || ! -f "$BUILD/.config" ]]; then
    make -C "$LINUX" O="$BUILD" tinyconfig
    "$LINUX/scripts/kconfig/merge_config.sh" -m -O "$BUILD" "$BUILD/.config" "$FRAGMENT"
    make -C "$LINUX" O="$BUILD" olddefconfig
    sha256sum "$FRAGMENT" | cut -d' ' -f1 > "$BUILD/cerynth-fragment.sha256"
fi
[[ "$(sha256sum "$FRAGMENT" | cut -d' ' -f1)" == "$(cat "$BUILD/cerynth-fragment.sha256" 2>/dev/null)" ]] || {
    echo 'error: fragment changed or build is unmanaged; use --reconfigure in the test build directory' >&2; exit 1;
}
CERYNTH_KERNEL_BUILD="$BUILD" JOBS="${JOBS:-4}" "$ROOT/scripts/build-kernel.sh" --image-only
