#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

DISK="${CERYNTH_VM_DISK:-$ROOT_DIR/vm/overlays/cerynth-dev.qcow2}"
SEED="${CERYNTH_CLOUD_INIT:-$ROOT_DIR/vm/cloud-init/seed.iso}"
MEMORY="${CERYNTH_VM_MEMORY:-6G}"
CPUS="${CERYNTH_VM_CPUS:-4}"
SSH_PORT="${CERYNTH_VM_SSH_PORT:-2222}"

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

[[ -f "$DISK" ]] || die "VM disk missing: $DISK"
[[ -f "$SEED" ]] || die "cloud-init seed missing: $SEED"

if [[ -r /dev/kvm && -w /dev/kvm ]]; then
    ACCEL=(-enable-kvm -cpu host)
else
    printf 'warning: KVM unavailable; using software emulation\n' >&2
    ACCEL=(-accel tcg -cpu max)
fi

mkdir -p "$ROOT_DIR/vm/logs"

LOG_FILE="$ROOT_DIR/vm/logs/base-boot-$(date +%Y%m%d-%H%M%S).log"

printf 'Starting Ubuntu base VM\n'
printf 'SSH after boot: ssh -i ~/.ssh/cerynth_vm -p %s cerynth@127.0.0.1\n' "$SSH_PORT"
printf 'Exit QEMU: Ctrl+A, then X\n\n'

qemu-system-x86_64 \
  -name CerynthOS-base \
  -machine q35 \
  "${ACCEL[@]}" \
  -smp "$CPUS" \
  -m "$MEMORY" \
  -drive "file=$DISK,format=qcow2,if=virtio" \
  -drive "file=$SEED,format=raw,if=virtio,readonly=on" \
  -device virtio-net-pci,netdev=net0 \
  -netdev "user,id=net0,hostfwd=tcp::$SSH_PORT-:22" \
  -nographic \
  2>&1 | tee "$LOG_FILE"
