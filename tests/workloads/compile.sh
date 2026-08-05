#!/usr/bin/env bash
# Usage: compile.sh <duration-seconds>
# Duration is advisory only (compilation runs to completion regardless) but
# is accepted for a uniform workload interface with the other scripts.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

require_tool cargo "install rustup: https://rustup.rs"

echo "compile: cargo build --workspace --release in ${REPO_ROOT}"
cd "${REPO_ROOT}"
cargo clean -p cerynth-telemetry >/dev/null 2>&1 || true
time cargo build --workspace --release
