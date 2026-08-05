#!/usr/bin/env bash
set -euo pipefail
FAIL=0

CONFIG_FILE="/boot/config-$(uname -r)"

if grep -q "^CONFIG_SCHED_CLASS_EXT=y" "$CONFIG_FILE"; then
    echo "OK    CONFIG_SCHED_CLASS_EXT=y"
else 
    echo "MISSING CONFIG-SCHED_CLASS_EXT"
    FAIL=1
fi

if [ -r /sys/kernel/btf/vmlinux ]; then
    echo "OK    BTF available"
else 
    echo "MISSING BTF (/sys/kernel/btf/vmlinux not readable)"
    FAIL=1
fi


if [ -r /sys/kernel/sched_ext/state ]; then
    echo "OK    sched_ext state file present (currently : $(cat /sys/kernel/sched_ext/state))"
else 
    echo "MISSING /sys/kernel/sched_ext/state not readable"
    FAIL=1
fi

exit "$FAIL"