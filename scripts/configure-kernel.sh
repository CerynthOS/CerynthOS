#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
LINUX="$ROOT/third_party/linux"
BUILD="$ROOT/kernel/build"

if [[ ! -d "$LINUX/.git" ]]; then
  echo "Linux source missing. Run: just fetch-upstream"
  exit 1
fi

mkdir -p "$BUILD"

if [[ -f "$ROOT/kernel/config/base.config" ]]; then
  cp "$ROOT/kernel/config/base.config" "$BUILD/.config"
else
  make -C "$LINUX" O="$BUILD" defconfig
fi

"$LINUX/scripts/config" --file "$BUILD/.config" \
  --enable BPF \
  --enable BPF_SYSCALL \
  --enable BPF_JIT \
  --enable BPF_JIT_ALWAYS_ON \
  --enable BPF_JIT_DEFAULT_ON \
  --enable DEBUG_INFO \
  --enable DEBUG_INFO_BTF \
  --enable SCHED_CLASS_EXT

make -C "$LINUX" O="$BUILD" olddefconfig

echo
echo "Relevant kernel options:"
grep -E \
  'CONFIG_(SCHED_CLASS_EXT|BPF|BPF_SYSCALL|BPF_JIT|DEBUG_INFO_BTF)=' \
  "$BUILD/.config"
