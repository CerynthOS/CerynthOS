#!/usr/bin/env bash
set -Eeuo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[[ -d "$ROOT_DIR/third_party/scx" ]] || { printf 'error: sched-ext/scx missing; run just fetch-upstream\n' >&2; exit 1; }
# shellcheck source=scripts/lib/source-pins.sh
source "$ROOT_DIR/scripts/lib/source-pins.sh"
require_pinned_source "$ROOT_DIR/third_party/scx" "$ROOT_DIR/schedulers/SCX_COMMIT"
cmp -s "$ROOT_DIR/schedulers/SCX_COMMIT" "$ROOT_DIR/third_party/scx.revision" || { printf 'error: SCX dependency pins differ\n' >&2; exit 1; }
cd "$ROOT_DIR"
cargo build --locked --release -p cerynth-scx
ls -lh "$ROOT_DIR/target/release/cerynth-scx"
