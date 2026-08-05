#!/usr/bin/env bash
set -euo pipefail

echo "== sched_ext state =="
cat /sys/kernel/sched_ext/state

echo
echo "== kernel =="
uname -r

echo
echo "== recent sched_ext kernel log =="
dmesg -T 2>/dev/null | grep -i sched_ext | tail -10
