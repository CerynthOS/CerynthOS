#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
LINUX="$ROOT/third_party/linux"
BUILD="$(realpath -m "${CERYNTH_KERNEL_BUILD:-$ROOT/kernel/build}")"

if ! git -C "$LINUX" rev-parse --git-dir >/dev/null 2>&1; then
  echo "Linux source missing. Run: just fetch-upstream"
  exit 1
fi
# shellcheck source=scripts/lib/source-pins.sh
source "$ROOT/scripts/lib/source-pins.sh"
require_pinned_source "$LINUX" "$ROOT/kernel/COMMIT"

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
  --enable SCHED_CLASS_EXT \
  --enable CGROUPS \
  --enable CGROUP_SCHED \
  --enable FAIR_GROUP_SCHED \
  --enable CFS_BANDWIDTH \
  --enable MEMCG \
  --enable BLK_CGROUP \
  --enable BLK_DEV_THROTTLING \
  --enable CGROUP_PIDS \
  --enable CPUSETS \
  --enable PSI \
  --disable SYSTEM_TRUSTED_KEYS \
  --disable SYSTEM_REVOCATION_KEYS

make -C "$LINUX" O="$BUILD" olddefconfig

echo
echo "Relevant kernel options:"
grep -E \
  'CONFIG_(SCHED_CLASS_EXT|BPF|BPF_SYSCALL|BPF_JIT|DEBUG_INFO_BTF|CGROUPS|CGROUP_SCHED|CFS_BANDWIDTH|MEMCG|BLK_CGROUP|CGROUP_PIDS|CPUSETS|PSI)=' \
  "$BUILD/.config"
