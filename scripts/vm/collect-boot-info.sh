#!/usr/bin/env bash
set -Eeuo pipefail

TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUTPUT_DIR="${1:-artifacts/boot-report/$TIMESTAMP}"

mkdir -p "$OUTPUT_DIR"

capture() {
    local name="$1"
    shift

    {
        printf '$'
        printf ' %q' "$@"
        printf '\n\n'
        "$@"
    } >"$OUTPUT_DIR/$name.txt" 2>&1 || true
}

capture uname uname -a
capture kernel-release uname -r
capture kernel-command-line cat /proc/cmdline
capture cpu lscpu
capture memory free -h
capture mounts findmnt
capture block-devices lsblk -f
capture modules lsmod
capture system-state systemctl is-system-running
capture failed-units systemctl --failed --no-pager
capture journal-warnings journalctl -b -p warning --no-pager
capture dmesg dmesg
capture dmesg-warnings dmesg --level=err,warn

if [[ -r /sys/kernel/btf/vmlinux ]]; then
    printf 'available\n' >"$OUTPUT_DIR/btf-status.txt"
else
    printf 'missing\n' >"$OUTPUT_DIR/btf-status.txt"
fi

if [[ -r /sys/kernel/sched_ext/state ]]; then
    capture sched-ext-state cat /sys/kernel/sched_ext/state
else
    printf 'unavailable\n' >"$OUTPUT_DIR/sched-ext-state.txt"
fi

if [[ -r "/boot/config-$(uname -r)" ]]; then
    cp "/boot/config-$(uname -r)" "$OUTPUT_DIR/kernel-config"
fi

cat >"$OUTPUT_DIR/summary.txt" <<SUMMARY
CerynthOS boot report
Timestamp: $TIMESTAMP
Hostname: $(hostname)
Kernel: $(uname -r)
System state: $(systemctl is-system-running 2>/dev/null || true)
BTF: $(cat "$OUTPUT_DIR/btf-status.txt")
sched_ext: $(cat "$OUTPUT_DIR/sched-ext-state.txt" 2>/dev/null || true)
SUMMARY

printf 'Boot report written to %s\n' "$OUTPUT_DIR"
