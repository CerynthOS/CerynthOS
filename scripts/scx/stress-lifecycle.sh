#!/usr/bin/env bash
#
# Repeatedly start, stop, re-profile and kill cerynth-scx, and check after
# every step that the kernel and the status files agree on what happened.
#
#   sudo ./scripts/scx/stress-lifecycle.sh --iterations 100
#
# One iteration:
#
#   start A -> SIGTERM -> start B -> change to C -> change to D -> SIGINT
#     -> restart A -> SIGKILL -> recover with B -> SIGTERM
#
# A-D rotate through all four profiles, so over four iterations every
# profile gets every role. A "profile change" is a stop followed by a start
# with the new --profile, which is exactly what cerynthd's ScxBackend does.
#
# Busy loops (one per CPU by default) keep the machine loaded throughout, so
# the scheduler always has real work while it's being started and killed.
#
# Results: artifacts/scx-lifecycle/<timestamp>/{progress.log,scheduler.log,
# kernel.log,result.json}. progress.log is synced after every iteration, so
# if the machine ever does hang, it shows exactly where.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=scripts/scx/test-lib.sh
source "$ROOT_DIR/scripts/scx/test-lib.sh"

ITERATIONS=100
LOAD=$(nproc)
HOLD=1.5
SCX_BIN=""
OUT_DIR=""

usage() {
    cat <<EOF
Usage: $0 [options]

  --iterations N   lifecycle iterations to run (default: $ITERATIONS)
  --load N         busy loops to run alongside (default: nproc = $LOAD, 0 = none)
  --hold SECONDS   how long each started scheduler runs before the next step (default: $HOLD)
  --binary PATH    cerynth-scx binary (default: target/release, else /usr/lib/cerynth)
  --log-dir DIR    where to write results (default: artifacts/scx-lifecycle/<timestamp>)
EOF
}

ARGS=("$@")
while (($#)); do
    case "$1" in
        --iterations) ITERATIONS="$2"; shift 2 ;;
        --load) LOAD="$2"; shift 2 ;;
        --hold) HOLD="$2"; shift 2 ;;
        --binary) SCX_BIN="$2"; shift 2 ;;
        --log-dir) OUT_DIR="$2"; shift 2 ;;
        -h | --help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

require_root_and_tools "${ARGS[@]}"
SCX_BIN="${SCX_BIN:-$(default_binary "$ROOT_DIR")}"
preflight

setup_log_dir "${OUT_DIR:-$ROOT_DIR/artifacts/scx-lifecycle/$(date +%Y%m%d-%H%M%S)}"

UNRECOVERABLE=false
cleanup() {
    stop_load
    reset_to_baseline || UNRECOVERABLE=true
    chown_log_dir
}
trap cleanup EXIT
trap 'log "interrupted"; exit 130' INT TERM

# run_for_a_while <profile>: let the scheduler work, then confirm it actually
# dispatched tasks and the machine still responds.
run_for_a_while() {
    sleep "$HOLD"
    check "$1: dispatching tasks (tasks_dispatched > 0)" stats_dispatching || return 1
    check "$1: machine responsive" probe_responsive || return 1
    check "$1: still healthy after ${HOLD}s" status_running_as "$1" || return 1
}

# start_and_run <profile>
start_and_run() { start_healthy "$1" && run_for_a_while "$1"; }

run_iteration() {
    local i="$1"
    local a="${PROFILES[$((i % 4))]}" b="${PROFILES[$(((i + 1) % 4))]}"
    local c="${PROFILES[$(((i + 2) % 4))]}" d="${PROFILES[$(((i + 3) % 4))]}"

    log "--- iteration $i/$ITERATIONS: $a -> $b -> $c -> $d"
    start_and_run "$a" || return 1
    stop_clean TERM || return 1

    start_and_run "$b" || return 1

    log "change profile $b -> $c"
    stop_clean TERM || return 1
    start_and_run "$c" || return 1

    log "change profile $c -> $d"
    stop_clean TERM || return 1
    start_and_run "$d" || return 1

    stop_clean INT || return 1

    log "restart with $a, then SIGKILL"
    start_and_run "$a" || return 1
    kill_hard KILL || return 1
    check "after SIGKILL: machine responsive" probe_responsive || return 1

    log "recover with $b"
    start_and_run "$b" || return 1
    stop_clean TERM || return 1
}

log "cerynth-scx lifecycle stress: $ITERATIONS iterations, load=$LOAD, hold=${HOLD}s"
log "binary: $SCX_BIN"
log "kernel: $(uname -r), sched_ext: $(scx_state), enable_seq: $(enable_seq)"

start_load "$LOAD"
START_SECONDS=$SECONDS

COMPLETED=0
FAILED_ITERATIONS=0
for ((i = 1; i <= ITERATIONS; i++)); do
    fails_before=$FAIL_COUNT
    # The kernel log is checked one iteration at a time: the kernel's log
    # buffer is too small to hold a whole 100-iteration run.
    mark_kernel_log

    if ! run_iteration "$i"; then
        log "iteration $i stopped early; scheduler log tail:"
        tail -5 "$SCHED_LOG" | sed 's/^/      /' | tee -a "$LOG_DIR/progress.log"
    fi
    if ! reset_to_baseline; then
        UNRECOVERABLE=true
        log "UNRECOVERABLE: sched_ext stuck in '$(scx_state)' even after SysRq-S. Stopping."
        break
    fi
    # No stall is expected anywhere in an iteration, including after the
    # SIGKILL: a "runnable task stall" here means the kernel watchdog had to
    # eject a scheduler that should have been gone already.
    check_kernel_log "iteration $i"

    ((FAIL_COUNT > fails_before)) && FAILED_ITERATIONS=$((FAILED_ITERATIONS + 1))
    COMPLETED=$i
    sync "$LOG_DIR/progress.log" 2>/dev/null || sync
done

stop_load
DURATION=$((SECONDS - START_SECONDS))
read -r FALLBACK_COUNT FALLBACK_MAX_MS FALLBACK_AVG_MS <<<"$(fallback_stats)"
# Launches that exited with EXIT_START_AGAIN and were repeated by
# start_healthy. Not a failure, but worth knowing how often it happens.
LOAD_RETRIES=$START_AGAIN_COUNT

write_result stress-lifecycle \
    --argjson iterations_requested "$ITERATIONS" \
    --argjson iterations_completed "$COMPLETED" \
    --argjson iterations_failed "$FAILED_ITERATIONS" \
    --argjson unrecoverable "$UNRECOVERABLE" \
    --argjson load "$LOAD" \
    --argjson duration_s "$DURATION" \
    --argjson final_enable_seq "$(enable_seq)" \
    --argjson hard_kills "$FALLBACK_COUNT" \
    --argjson fallback_ms_max "$FALLBACK_MAX_MS" \
    --argjson fallback_ms_avg "$FALLBACK_AVG_MS" \
    --argjson load_retries "$LOAD_RETRIES"

log "done in ${DURATION}s: $COMPLETED/$ITERATIONS iterations, $FAILED_ITERATIONS failed," \
    "$PASS_COUNT checks passed, $FAIL_COUNT failed"
log "fallback after SIGKILL: max ${FALLBACK_MAX_MS}ms, avg ${FALLBACK_AVG_MS}ms over $FALLBACK_COUNT kills;" \
    "load retries: $LOAD_RETRIES"
log "results: $LOG_DIR/result.json"

if ((FAIL_COUNT > 0)) || [[ "$UNRECOVERABLE" == true ]]; then
    exit 1
fi
