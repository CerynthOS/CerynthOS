#!/usr/bin/env bash
set -Eeuo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
failed=0
check() {
    if command -v "$1" >/dev/null 2>&1; then
        printf '[OK]   %s\n' "$1"
    else
        printf '[FAIL] %s\n' "$1"
        failed=1
    fi
}
for tool in git cargo rustc clang make gcc bc pkg-config qemu-system-x86_64 qemu-img ssh rsync jq curl shellcheck cloud-localds; do check "$tool"; done
# Kernel build toolchain used by scripts/configure-kernel.sh and scripts/build-kernel.sh.
# pahole (dwarves) and libdw are both needed for CONFIG_DEBUG_INFO_BTF and
# CONFIG_MODVERSIONS on the 7.x tree (scripts/gendwarfksyms uses libdw).
for tool in bison flex pahole zstd cpio; do check "$tool"; done
[[ -f "$ROOT_DIR/third_party/linux/Makefile" ]] && printf '[OK]   Linux source\n' || printf '[WARN] Linux source missing\n'
if [[ -f "$ROOT_DIR/third_party/linux/Makefile" && -f "$ROOT_DIR/kernel/COMMIT" ]]; then
    pinned="$(tr -d '[:space:]' < "$ROOT_DIR/kernel/COMMIT")"
    actual="$(git -C "$ROOT_DIR/third_party/linux" rev-parse HEAD 2>/dev/null || true)"
    if [[ -n "$pinned" && "$pinned" == "$actual" ]]; then
        printf '[OK]   Linux source at pinned commit %.12s\n' "$pinned"
    else
        printf '[FAIL] Linux source at %s, kernel/COMMIT pins %.12s (run: just fetch-upstream)\n' "${actual:0:12}" "${pinned:-unset}"
        failed=1
    fi
fi
[[ -f "$ROOT_DIR/kernel/build/.config" ]] && printf '[OK]   Kernel configured (kernel/build/.config)\n' || printf '[WARN] Kernel not configured yet (just kernel-config)\n'
[[ -d "$ROOT_DIR/third_party/scx/rust" ]] && printf '[OK]   sched-ext/scx source\n' || printf '[WARN] sched-ext/scx source missing\n'
[[ -e /dev/kvm ]] && printf '[OK]   /dev/kvm exists\n' || printf '[WARN] /dev/kvm unavailable\n'
[[ -f "$ROOT_DIR/vm/generated/seed.iso" ]] && printf '[OK]   cloud-init seed\n' || printf '[WARN] cloud-init seed missing\n'
[[ -f "$ROOT_DIR/vm/root-filesystem.env" ]] && printf '[OK]   root filesystem config\n' || printf '[WARN] root filesystem config missing\n'
exit "$failed"
