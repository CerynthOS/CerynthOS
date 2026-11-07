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
[[ -f "$ROOT_DIR/third_party/linux/Makefile" ]] && printf '[OK]   Linux source\n' || printf '[WARN] Linux source missing\n'
[[ -d "$ROOT_DIR/third_party/scx/rust" ]] && printf '[OK]   sched-ext/scx source\n' || printf '[WARN] sched-ext/scx source missing\n'
[[ -e /dev/kvm ]] && printf '[OK]   /dev/kvm exists\n' || printf '[WARN] /dev/kvm unavailable\n'
[[ -f "$ROOT_DIR/vm/generated/seed.iso" ]] && printf '[OK]   cloud-init seed\n' || printf '[WARN] cloud-init seed missing\n'
[[ -f "$ROOT_DIR/vm/root-filesystem.env" ]] && printf '[OK]   root filesystem config\n' || printf '[WARN] root filesystem config missing\n'
exit "$failed"
