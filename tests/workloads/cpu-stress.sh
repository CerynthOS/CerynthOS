#!/usr/bin/env bash
# Usage: cpu-stress.sh <duration-seconds>
# Saturates all CPUs for the given duration.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

DURATION="${1:-30}"
require_tool stress-ng "sudo apt install -y stress-ng"

cpus="$(nproc)"
echo "cpu-stress: stress-ng --cpu ${cpus} --timeout ${DURATION}s"
exec stress-ng --cpu "${cpus}" --timeout "${DURATION}s" --metrics-brief
