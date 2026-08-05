#!/usr/bin/env bash
# Usage: burst.sh <duration-seconds>
# Alternates short CPU bursts with idle pauses to approximate bursty
# interactive/background workloads rather than sustained load.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

DURATION="${1:-30}"
require_tool stress-ng "sudo apt install -y stress-ng"

cpus="$(nproc)"
end=$(( $(date +%s) + DURATION ))

echo "burst: alternating stress-ng --cpu ${cpus} bursts and idle pauses for ${DURATION}s"
while [ "$(date +%s)" -lt "${end}" ]; do
  stress-ng --cpu "${cpus}" --timeout 3s >/dev/null 2>&1 || true
  sleep 2
done
