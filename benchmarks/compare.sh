#!/usr/bin/env bash
# Usage: benchmarks/compare.sh [date]
#
# Reads every artifacts/benchmarks/<date>/<profile>/<workload>/summary.json
# and writes a single comparison report to:
#   artifacts/benchmarks/<date>/report.md
#   artifacts/benchmarks/latest/report.md   (copy, for a stable link)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

DATE_DIR="${1:-$(date +%Y-%m-%d)}"
RUN_DIR="${REPO_ROOT}/artifacts/benchmarks/${DATE_DIR}"

if [ ! -d "${RUN_DIR}" ]; then
  echo "error: no runs found at ${RUN_DIR}" >&2
  exit 1
fi

REPORT="${RUN_DIR}/report.md"

python3 - "${RUN_DIR}" "${DATE_DIR}" "${REPORT}" <<'PYEOF'
import json
import sys
from pathlib import Path

run_dir, date_dir, report_path = sys.argv[1:4]
run_dir = Path(run_dir)

results = {}  # workload -> profile -> summary
for profile_dir in sorted(p for p in run_dir.iterdir() if p.is_dir()):
    profile = profile_dir.name
    for workload_dir in sorted(w for w in profile_dir.iterdir() if w.is_dir()):
        workload = workload_dir.name
        summary_file = workload_dir / "summary.json"
        metadata_file = workload_dir / "metadata.json"
        if not summary_file.exists():
            continue
        summary = json.loads(summary_file.read_text())
        metadata = json.loads(metadata_file.read_text()) if metadata_file.exists() else {}
        results.setdefault(workload, {})[profile] = {**summary, **{
            "sched_ext_state_start": metadata.get("sched_ext_state_start"),
            "sched_ext_state_end": metadata.get("sched_ext_state_end"),
        }}

lines = [f"# Benchmark comparison — {date_dir}", ""]
if not results:
    lines.append("No benchmark runs found for this date.")
else:
    for workload, by_profile in sorted(results.items()):
        lines.append(f"## {workload}")
        lines.append("")
        lines.append("| profile | avg cpu% | max cpu% | avg load(1m) | ctxt switches | interrupts | sched_ext |")
        lines.append("|---|---|---|---|---|---|---|")
        for profile, s in sorted(by_profile.items()):
            if "error" in s:
                lines.append(f"| {profile} | - | - | - | - | - | {s.get('sched_ext_state_start')} |")
                continue
            sched = f"{s.get('sched_ext_state_start')}→{s.get('sched_ext_state_end')}"
            lines.append(
                f"| {profile} | {s['avg_cpu_usage_percent']} | {s['max_cpu_usage_percent']} | "
                f"{s['avg_load_1m']} | {s['context_switches_delta']} | {s['interrupts_delta']} | {sched} |"
            )
        lines.append("")

lines.append("_Reproducible baselines only — no performance conclusions are implied by these numbers alone._")

Path(report_path).write_text("\n".join(lines) + "\n")
print(f"wrote {report_path}")
PYEOF

mkdir -p "${REPO_ROOT}/artifacts/benchmarks/latest"
cp "${REPORT}" "${REPO_ROOT}/artifacts/benchmarks/latest/report.md"
echo "==> also copied to ${REPO_ROOT}/artifacts/benchmarks/latest/report.md"
