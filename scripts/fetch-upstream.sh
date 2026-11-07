#!/usr/bin/env bash
set -Eeuo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
THIRD_PARTY="$ROOT_DIR/third_party"
LINUX_DIR="$THIRD_PARTY/linux"
SCX_DIR="$THIRD_PARTY/scx"
LINUX_COMMIT="$(tr -d '[:space:]' < "$ROOT_DIR/kernel/COMMIT")"
SCX_COMMIT="$(tr -d '[:space:]' < "$ROOT_DIR/schedulers/SCX_COMMIT")"
mkdir -p "$THIRD_PARTY"
if [[ ! -d "$LINUX_DIR/.git" ]]; then git clone https://github.com/torvalds/linux.git "$LINUX_DIR"; fi
git -C "$LINUX_DIR" fetch origin
git -C "$LINUX_DIR" checkout --detach "$LINUX_COMMIT"
if [[ ! -d "$SCX_DIR/.git" ]]; then git clone https://github.com/sched-ext/scx.git "$SCX_DIR"; fi
git -C "$SCX_DIR" fetch origin
git -C "$SCX_DIR" checkout --detach "$SCX_COMMIT"
printf 'Linux: %s\n' "$(git -C "$LINUX_DIR" rev-parse HEAD)"
printf 'SCX:   %s\n' "$(git -C "$SCX_DIR" rev-parse HEAD)"
