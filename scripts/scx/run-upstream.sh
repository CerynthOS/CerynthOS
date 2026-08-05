#! /usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
BINARY="${ROOT}/third_party/scx/target/release/${1:-scx_rustland}"

if [[ ! -x "$BINARY" ]]; then
    echo "ERROR: $BINARY not found or not executable" >&2
    exit 1
fi

"$ROOT/scripts/scx/check-support.sh"

PID_FILE="/tmp/cerynth-scx.pid"
LOG_FILE="/tmp/cerynth-scx.log"

sudo -v

echo "Starting $BINARY (logs: $LOG_FILE)"
sudo nohup "$BINARY" > "$LOG_FILE" 2>&1 &
SCX_PID=$!
echo "$SCX_PID" > "$PID_FILE"

READY=0
for _ in $(seq 1 25); do
  if [ "$(cat /sys/kernel/sched_ext/state)" = "enabled" ]; then
    READY=1
    break
  fi
  sleep 0.2
done

if [ "$READY" -eq 1 ]; then
  echo "Launched - PID $SCX_PID, pidfile $PID_FILE"
else
  echo "ERROR: scheduler did not enable within 5s. Last log lines:" >&2
  tail -5 "$LOG_FILE" >&2
  exit 1
fi