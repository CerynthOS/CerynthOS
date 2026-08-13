#!/usr/bin/env bash
set -euo pipefail

echo "Stopping any running Cerynth/SCX scheduler..."

# cerynth-scx is the scheduler launched by CerynthOS.
# scx_* covers upstream SCX schedulers such as scx_rustland.
sudo pkill -INT -f "target/release/cerynth-scx" || true
sudo pkill -INT -f "target/release/scx_" || true

for _ in $(seq 1 25); do
    if [ "$(cat /sys/kernel/sched_ext/state 2>/dev/null || echo unavailable)" = "disabled" ]; then
        echo "sched_ext state: disabled"
        exit 0
    fi
    sleep 0.2
done

echo "ERROR: sched_ext did not become disabled within 5s." >&2
cat /sys/kernel/sched_ext/state >&2
exit 1
