#!/usr/bin/env bash
# Usage: idle-baseline.sh <duration-seconds>
# Induces no load; establishes a resting baseline for comparison.
set -euo pipefail
DURATION="${1:-30}"
echo "idle-baseline: sleeping ${DURATION}s with no induced load"
sleep "${DURATION}"
