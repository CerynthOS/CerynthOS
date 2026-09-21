#!/usr/bin/env bash
set -Eeuo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[[ -d "$ROOT_DIR/third_party/scx" ]] || { printf 'error: sched-ext/scx missing; run just fetch-upstream\n' >&2; exit 1; }
cd "$ROOT_DIR"
cargo build --release -p cerynth-scx
ls -lh "$ROOT_DIR/target/release/cerynth-scx"
