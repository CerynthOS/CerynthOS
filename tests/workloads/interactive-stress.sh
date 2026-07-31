#!/usr/bin/env bash
# Usage: interactive-stress.sh <duration-seconds>
# Background CPU load plus a foreground loop of short-lived processes,
# approximating an interactive session competing with a busy background job.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

DURATION="${1:-30}"
require_tool stress-ng "sudo apt install -y stress-ng"

cpus="$(nproc)"
background_load=$(( cpus > 1 ? cpus - 1 : 1 ))

echo "interactive-stress: background stress-ng --cpu ${background_load} + foreground bursts for ${DURATION}s"
stress-ng --cpu "${background_load}" --timeout "${DURATION}s" &
bg_pid=$!

end=$(( $(date +%s) + DURATION ))
while [ "$(date +%s)" -lt "${end}" ]; do
  /bin/true
  sleep 0.2
done

wait "${bg_pid}"
