#!/usr/bin/env bash
# CPU pressure plus process churn for a bounded mixed workload.
set -euo pipefail
DURATION="${1:-60}"
command -v stress-ng >/dev/null || { echo 'Install stress-ng first' >&2; exit 1; }
stress-ng --cpu 1 --timeout "${DURATION}s" --metrics-brief &
LOAD_PID=$!
trap 'kill "$LOAD_PID" 2>/dev/null || true; wait "$LOAD_PID" 2>/dev/null || true' EXIT
END=$((SECONDS + DURATION))
ITERATIONS=0
while (( SECONDS < END )); do
  /bin/true
  ITERATIONS=$((ITERATIONS + 1))
  sleep 0.05
done
wait "$LOAD_PID"
echo "interactive_iterations=${ITERATIONS}"
