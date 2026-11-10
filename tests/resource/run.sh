#!/usr/bin/env bash
# Run the cgroup v2 resource-control tests and print the support matrix.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"

echo "== cgroup v2 support matrix =="
cargo run --quiet -p cerynth-resources --bin cerynth-resource-support

echo
echo "== resource tests =="
cargo test -p cerynth-resources -- --nocapture
