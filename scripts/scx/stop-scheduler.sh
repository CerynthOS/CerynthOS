#!/usr/bin/env bash
set -euo pipefail

echo "Stopping any running scx scheduler..."
sudo pkill -INT -f "target/release/scx_" || echo "Nothing was running."

sleep 1
echo "sched_ext state: $(cat /sys/kernel/sched_ext/state)"
