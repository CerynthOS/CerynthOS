#!/usr/bin/env bash
#
# Recover a CerynthOS VM whose runtime has wedged.
#
# Returns the VM to a known-good state: no scheduler attached, Linux back in
# charge of scheduling, no stale sockets, and cerynthd freshly started.
#
#   ./scripts/vm/runtime-recovery.sh
#   ./scripts/vm/runtime-recovery.sh --no-restart   # stop everything, stay down

set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# shellcheck source=scripts/vm/cerynth-vm-common.sh
source "$ROOT_DIR/scripts/vm/cerynth-vm-common.sh"

RESTART=1

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-restart) RESTART=0; shift ;;
        -h|--help)
            printf 'Usage: %s [--no-restart]\n' "$0"
            exit 0
            ;;
        *) die "unknown argument: $1" ;;
    esac
done

require_vm_key
require_vm_reachable
require_vm_sudo

vm_bash "$RESTART" <<'REMOTE'
set -uo pipefail

RESTART="$1"

echo "=== CerynthOS Runtime Recovery ==="

echo
echo "--- stopping daemon ---"
sudo systemctl stop cerynthd 2>/dev/null || true

echo
echo "--- stopping scheduler ---"
# SIGTERM first: cerynth-scx detaches from sched_ext on a clean shutdown,
# which is what hands scheduling back to Linux.
if pgrep -x cerynth-scx >/dev/null 2>&1; then
    sudo pkill -TERM -x cerynth-scx || true

    for _ in $(seq 1 20); do
        pgrep -x cerynth-scx >/dev/null 2>&1 || break
        sleep 0.5
    done

    if pgrep -x cerynth-scx >/dev/null 2>&1; then
        echo "scheduler ignored SIGTERM, sending SIGKILL"
        sudo pkill -KILL -x cerynth-scx || true
        sleep 1
    fi
else
    echo "no cerynth-scx process running"
fi

echo
echo "--- sched_ext state ---"
STATE="$(cat /sys/kernel/sched_ext/state 2>/dev/null || echo unavailable)"
echo "$STATE"

if [ "$STATE" = "enabled" ]; then
    echo "WARNING: a BPF scheduler is still attached."
    echo "The kernel detaches it when the loading process exits; if this"
    echo "persists, reboot the VM to guarantee a return to Linux scheduling."
else
    echo "Linux scheduling is in control."
fi

echo
echo "--- removing stale runtime files ---"
sudo rm -f /run/cerynth/cerynthd.sock /run/cerynth/scheduler.json
sudo install -d -m 0755 /run/cerynth

echo
echo "--- clearing failed unit state ---"
sudo systemctl reset-failed cerynthd 2>/dev/null || true

if [ "$RESTART" = "1" ]; then
    echo
    echo "--- starting daemon ---"
    sudo systemctl start cerynthd

    sleep 1

    echo
    echo "--- daemon status ---"
    systemctl --no-pager --full status cerynthd || true

    echo
    echo "--- socket ---"
    if [ -S /run/cerynth/cerynthd.sock ]; then
        echo "IPC socket present"
    else
        echo "IPC socket MISSING"
    fi
else
    echo
    echo "--- leaving daemon stopped (--no-restart) ---"
fi

echo
echo "--- recent logs ---"
sudo journalctl -u cerynthd -b -n 40 --no-pager || true
REMOTE

printf '\nRecovery complete.\n'
printf 'Verify with: just vm-smoke\n'
