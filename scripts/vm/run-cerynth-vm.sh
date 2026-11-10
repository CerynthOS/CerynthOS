#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

LINUX_SRC="$ROOT_DIR/third_party/linux"
KERNEL_BUILD="$ROOT_DIR/kernel/build"
VM_DISK="${CERYNTH_VM_DISK:-$ROOT_DIR/vm/overlays/cerynth-dev.qcow2}"

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

command -v qemu-system-x86_64 >/dev/null ||
    die "qemu-system-x86_64 is not installed"

[[ -f "$LINUX_SRC/Makefile" ]] ||
    die "Linux source tree missing: $LINUX_SRC"

KERNEL_RELEASE="$(
    make -s -C "$LINUX_SRC" \
        O="$KERNEL_BUILD" \
        kernelrelease
)"

KERNEL_IMAGE="${CERYNTH_KERNEL_IMAGE:-$KERNEL_BUILD/arch/x86/boot/bzImage}"
INITRD_IMAGE="${CERYNTH_INITRD_IMAGE:-$ROOT_DIR/vm/initramfs/initrd.img-$KERNEL_RELEASE}"

MEMORY="${CERYNTH_VM_MEMORY:-6G}"
CPUS="${CERYNTH_VM_CPUS:-4}"
SSH_PORT="${CERYNTH_VM_SSH_PORT:-2222}"

[[ -f "$KERNEL_IMAGE" ]] ||
    die "kernel image missing: $KERNEL_IMAGE"

[[ -f "$INITRD_IMAGE" ]] ||
    die "initramfs missing: $INITRD_IMAGE"

[[ -f "$VM_DISK" ]] ||
    die "VM disk missing: $VM_DISK"

ROOT_SPEC=""

if [[ -n "${CERYNTH_ROOT_UUID:-}" ]]; then
    ROOT_SPEC="UUID=$CERYNTH_ROOT_UUID"
elif [[ -n "${CERYNTH_ROOT_DEVICE:-}" ]]; then
    ROOT_SPEC="$CERYNTH_ROOT_DEVICE"
elif [[ -f "$ROOT_DIR/vm/root-filesystem.env" ]]; then
    # shellcheck disable=SC1091
    source "$ROOT_DIR/vm/root-filesystem.env"

    if [[ -n "${CERYNTH_ROOT_UUID:-}" ]]; then
        ROOT_SPEC="UUID=$CERYNTH_ROOT_UUID"
    else
        ROOT_SPEC="${CERYNTH_ROOT_DEVICE:-/dev/vda1}"
    fi
else
    ROOT_SPEC="/dev/vda1"
fi

if [[ -r /dev/kvm && -w /dev/kvm ]]; then
    ACCEL=(-enable-kvm -cpu host)
    ACCEL_NAME="KVM"
else
    ACCEL=(-accel tcg -cpu max)
    ACCEL_NAME="TCG"
fi

mkdir -p "$ROOT_DIR/vm/logs"

TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
LOG_FILE="$ROOT_DIR/vm/logs/cerynth-boot-$TIMESTAMP.log"

KERNEL_CMDLINE="$(
    printf '%s' \
      "root=$ROOT_SPEC rw rootwait " \
      "console=ttyS0,115200n8 " \
      "loglevel=7 " \
      "systemd.show_status=1 " \
      "panic=10"
)"

printf '\nCerynthOS direct kernel boot\n'
printf '%-14s %s\n' "Kernel:" "$KERNEL_IMAGE"
printf '%-14s %s\n' "Release:" "$KERNEL_RELEASE"
printf '%-14s %s\n' "Initramfs:" "$INITRD_IMAGE"
printf '%-14s %s\n' "Disk:" "$VM_DISK"
printf '%-14s %s\n' "Root:" "$ROOT_SPEC"
printf '%-14s %s\n' "Acceleration:" "$ACCEL_NAME"
printf '%-14s %s\n' "CPUs:" "$CPUS"
printf '%-14s %s\n' "Memory:" "$MEMORY"
printf '%-14s localhost:%s\n' "SSH:" "$SSH_PORT"
printf '%-14s %s\n\n' "Boot log:" "$LOG_FILE"

printf 'Exit QEMU using Ctrl+A, then X\n\n'

qemu-system-x86_64 \
  -name CerynthOS-dev \
  -machine q35 \
  "${ACCEL[@]}" \
  -smp "$CPUS" \
  -m "$MEMORY" \
  -kernel "$KERNEL_IMAGE" \
  -initrd "$INITRD_IMAGE" \
  -drive "file=$VM_DISK,format=qcow2,if=virtio" \
  -device virtio-net-pci,netdev=net0 \
  -netdev "user,id=net0,hostfwd=tcp::$SSH_PORT-:22" \
  -append "$KERNEL_CMDLINE" \
  -no-reboot \
  -nographic \
  2>&1 | tee "$LOG_FILE"
