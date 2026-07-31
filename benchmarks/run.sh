#!/usr/bin/env bash
# Usage: benchmarks/run.sh --profile <profile> --workload <workload> --duration <seconds>
#
# Runs a workload from tests/workloads/ while recording telemetry, and
# writes a self-contained artifact directory:
#
#   artifacts/benchmarks/<date>/<profile>/<workload>/
#     metadata.json
#     telemetry.jsonl
#     workload.log
#     summary.json
#     summary.md
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORKLOAD_DIR="${REPO_ROOT}/tests/workloads"

PROFILE=""
WORKLOAD=""
DURATION=30

while [ $# -gt 0 ]; do
  case "$1" in
    --profile) PROFILE="$2"; shift 2 ;;
    --workload) WORKLOAD="$2"; shift 2 ;;
    --duration) DURATION="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
done

if [ -z "${PROFILE}" ] || [ -z "${WORKLOAD}" ]; then
  echo "usage: $0 --profile <profile> --workload <workload> --duration <seconds>" >&2
  exit 1
fi

WORKLOAD_SCRIPT="${WORKLOAD_DIR}/${WORKLOAD}.sh"
if [ ! -x "${WORKLOAD_SCRIPT}" ]; then
  echo "error: no workload script at ${WORKLOAD_SCRIPT}" >&2
  echo "available workloads:" >&2
  ls "${WORKLOAD_DIR}"/*.sh 2>/dev/null | xargs -n1 basename | sed 's/\.sh$//' >&2
  exit 1
fi

DATE_DIR="$(date +%Y-%m-%d)"
OUT_DIR="${REPO_ROOT}/artifacts/benchmarks/${DATE_DIR}/${PROFILE}/${WORKLOAD}"
mkdir -p "${OUT_DIR}"

echo "==> writing artifacts to ${OUT_DIR}"

source "${HOME}/.cargo/env" 2>/dev/null || true
echo "==> building telemetry recorder (release)"
cargo build --release -p cerynth-telemetry --bin cerynth-telemetry-record --manifest-path "${REPO_ROOT}/Cargo.toml" >/dev/null
RECORDER="${REPO_ROOT}/target/release/cerynth-telemetry-record"

SCHED_EXT_STATE_START="$(cat /sys/kernel/sched_ext/state 2>/dev/null || echo unavailable)"
KERNEL_RELEASE="$(uname -r)"
CPU_MODEL="$(grep -m1 'model name' /proc/cpuinfo 2>/dev/null | sed 's/model name\s*:\s*//' || true)"
if [ -z "${CPU_MODEL}" ] || [ "${CPU_MODEL}" = "-" ]; then
  # x86 /proc/cpuinfo has no "model name" on some arches (e.g. aarch64);
  # fall back to lscpu's vendor/architecture fields.
  VENDOR="$(lscpu 2>/dev/null | awk -F: '/^Vendor ID:/ {gsub(/^[ \t]+/, "", $2); print $2}')"
  ARCH="$(uname -m)"
  CPU_MODEL="${VENDOR:-unknown} (${ARCH})"
fi
CPU_COUNT="$(nproc)"
START_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
START_EPOCH="$(date +%s)"

# Give the recorder a small buffer over the workload duration so it fully
# brackets the workload's start and end.
RECORD_DURATION=$(( DURATION + 5 ))

"${RECORDER}" \
  --interval-ms 1000 \
  --duration-seconds "${RECORD_DURATION}" \
  --output "${OUT_DIR}/telemetry.jsonl" \
  > "${OUT_DIR}/telemetry.log" 2>&1 &
RECORDER_PID=$!

sleep 1  # let the recorder take its baseline sample before load starts

echo "==> running workload: ${WORKLOAD} (${DURATION}s)"
WORKLOAD_EXIT=0
"${WORKLOAD_SCRIPT}" "${DURATION}" > "${OUT_DIR}/workload.log" 2>&1 || WORKLOAD_EXIT=$?

wait "${RECORDER_PID}" || true

END_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
END_EPOCH="$(date +%s)"
SCHED_EXT_STATE_END="$(cat /sys/kernel/sched_ext/state 2>/dev/null || echo unavailable)"

python3 - "${OUT_DIR}" "${PROFILE}" "${WORKLOAD}" "${DURATION}" "${WORKLOAD_EXIT}" \
  "${KERNEL_RELEASE}" "${CPU_MODEL}" "${CPU_COUNT}" \
  "${SCHED_EXT_STATE_START}" "${SCHED_EXT_STATE_END}" \
  "${START_TS}" "${END_TS}" "${START_EPOCH}" "${END_EPOCH}" <<'PYEOF'
import json
import sys

(out_dir, profile, workload, duration, workload_exit,
 kernel_release, cpu_model, cpu_count,
 sched_ext_start, sched_ext_end,
 start_ts, end_ts, start_epoch, end_epoch) = sys.argv[1:15]

telemetry_path = f"{out_dir}/telemetry.jsonl"
snapshots = []
with open(telemetry_path) as f:
    for line in f:
        line = line.strip()
        if line:
            snapshots.append(json.loads(line))

metadata = {
    "profile": profile,
    "workload": workload,
    "requested_duration_seconds": int(duration),
    "total_wall_seconds": int(end_epoch) - int(start_epoch),
    "workload_exit_code": int(workload_exit),
    "kernel_release": kernel_release,
    "cpu_model": cpu_model,
    "cpu_count": int(cpu_count),
    "sched_ext_state_start": sched_ext_start,
    "sched_ext_state_end": sched_ext_end,
    "started_at": start_ts,
    "ended_at": end_ts,
    "sample_count": len(snapshots),
}
with open(f"{out_dir}/metadata.json", "w") as f:
    json.dump(metadata, f, indent=2)
    f.write("\n")

if snapshots:
    cpu_values = [s["cpu_usage_percent"] for s in snapshots]
    load_values = [s["load_1m"] for s in snapshots]
    mem_values = [s["memory_used_bytes"] for s in snapshots]
    ctxt_delta = snapshots[-1]["context_switches"] - snapshots[0]["context_switches"]
    intr_delta = snapshots[-1]["interrupts"] - snapshots[0]["interrupts"]
    summary = {
        "avg_cpu_usage_percent": round(sum(cpu_values) / len(cpu_values), 2),
        "max_cpu_usage_percent": round(max(cpu_values), 2),
        "avg_load_1m": round(sum(load_values) / len(load_values), 2),
        "avg_memory_used_bytes": int(sum(mem_values) / len(mem_values)),
        "context_switches_delta": ctxt_delta,
        "interrupts_delta": intr_delta,
        "sample_count": len(snapshots),
    }
else:
    summary = {"error": "no telemetry samples recorded"}

with open(f"{out_dir}/summary.json", "w") as f:
    json.dump(summary, f, indent=2)
    f.write("\n")

with open(f"{out_dir}/summary.md", "w") as f:
    f.write(f"# Benchmark: {profile} / {workload}\n\n")
    f.write(f"- Kernel: {kernel_release}\n")
    f.write(f"- CPU: {cpu_model} ({cpu_count} logical CPUs)\n")
    f.write(f"- sched_ext state: {sched_ext_start} -> {sched_ext_end}\n")
    f.write(f"- Duration: requested {duration}s, total wall time {metadata['total_wall_seconds']}s (includes telemetry buffer)\n")
    f.write(f"- Workload exit code: {workload_exit}\n\n")
    if snapshots:
        f.write("| metric | value |\n|---|---|\n")
        for key, value in summary.items():
            f.write(f"| {key} | {value} |\n")
    else:
        f.write("No telemetry samples were recorded.\n")
PYEOF

echo "==> done"
echo "    metadata:  ${OUT_DIR}/metadata.json"
echo "    telemetry: ${OUT_DIR}/telemetry.jsonl"
echo "    summary:   ${OUT_DIR}/summary.md"

exit "${WORKLOAD_EXIT}"
