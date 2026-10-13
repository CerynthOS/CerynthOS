#!/usr/bin/env bash
#
# Break cerynth-scx on purpose, in every way the plan lists, and check that
# the kernel's default scheduler always catches the machine.
#
#   sudo ./scripts/scx/fault-test.sh            # every scenario
#   sudo ./scripts/scx/fault-test.sh --only heartbeat-freeze,sysrq-disable
#   ./scripts/scx/fault-test.sh --list
#
# After each scenario the machine is returned to "no scheduler loaded" and a
# fresh scheduler must start and dispatch cleanly - a fault that leaves the
# system unable to run the scheduler again counts as a failure.
#
# Results: artifacts/scx-faults/<timestamp>/{progress.log,scheduler.log,
# kernel.log,result.json}.

# shellcheck disable=SC2016  # $names inside jq programs are jq's, not the shell's

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=scripts/scx/test-lib.sh
source "$ROOT_DIR/scripts/scx/test-lib.sh"

LOAD=$(nproc)
SCX_BIN=""
OUT_DIR=""
ONLY=""

SCENARIOS=(
    crash-sigkill
    crash-sigsegv
    status-deleted
    status-corrupt
    heartbeat-freeze
    dispatch-errors
    dispatch-stall
    sysrq-disable
)

declare -A DESCRIPTIONS=(
    [crash-sigkill]="scheduler killed outright (SIGKILL)"
    [crash-sigsegv]="scheduler crashes (SIGSEGV)"
    [status-deleted]="status and stats files deleted while running"
    [status-corrupt]="status and stats files overwritten with invalid JSON"
    [heartbeat-freeze]="scheduler frozen (SIGSTOP): heartbeat stops, kernel watchdog must eject it"
    [dispatch-errors]="1 in 50 dispatches fails: must retry and stay healthy"
    [dispatch-stall]="every dispatch fails: must give up on its own before the kernel watchdog"
    [sysrq-disable]="sched_ext disabled underneath the scheduler (SysRq-S)"
)

# Kernel messages a scenario causes on purpose, so they don't count as
# trouble.
declare -A EXPECTED_KERNEL=(
    [heartbeat-freeze]="runnable task stall"
)

usage() {
    cat <<EOF
Usage: $0 [options]

  --only A,B       run only these scenarios (see --list)
  --list           list scenarios and exit
  --load N         busy loops to run alongside (default: nproc = $LOAD, min 1)
  --binary PATH    cerynth-scx binary (default: target/release, else /usr/lib/cerynth)
  --log-dir DIR    where to write results (default: artifacts/scx-faults/<timestamp>)
EOF
}

ARGS=("$@")
while (($#)); do
    case "$1" in
        --only) ONLY="$2"; shift 2 ;;
        --list)
            for s in "${SCENARIOS[@]}"; do printf '%-18s %s\n' "$s" "${DESCRIPTIONS[$s]}"; done
            exit 0
            ;;
        --load) LOAD="$2"; shift 2 ;;
        --binary) SCX_BIN="$2"; shift 2 ;;
        --log-dir) OUT_DIR="$2"; shift 2 ;;
        -h | --help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if [[ -n "$ONLY" ]]; then
    IFS=, read -ra SELECTED <<<"$ONLY"
    for s in "${SELECTED[@]}"; do
        if [[ -z "${DESCRIPTIONS[$s]+x}" ]]; then
            echo "unknown scenario: $s (see --list)" >&2
            exit 2
        fi
    done
else
    SELECTED=("${SCENARIOS[@]}")
fi

# The freeze scenario only stalls if tasks actually need the user-space
# scheduler; on an idle machine they could all be placed on idle CPUs by the
# BPF side alone.
((LOAD < 1)) && LOAD=1

require_root_and_tools "${ARGS[@]}"
SCX_BIN="${SCX_BIN:-$(default_binary "$ROOT_DIR")}"
preflight

setup_log_dir "${OUT_DIR:-$ROOT_DIR/artifacts/scx-faults/$(date +%Y%m%d-%H%M%S)}"

cleanup() {
    stop_load
    reset_to_baseline || true
    chown_log_dir
}
trap cleanup EXIT
trap 'log "interrupted"; exit 130' INT TERM

# reap_scheduler: collect the (already exited) scheduler's exit code into
# SCX_RC. Has to run in this shell, not a $(...) subshell: only the parent
# can wait for a child.
SCX_RC=0
reap_scheduler() {
    SCX_RC=0
    wait "$SCX_PID" 2>/dev/null || SCX_RC=$?
    SCX_PID=""
}

kernel_says() { kernel_log_since_mark | grep -q "$1"; }
kernel_silent_about() { ! kernel_says "$1"; }

scenario_crash() {
    local sig="$1"
    start_healthy balanced || return 1
    sleep 1
    kill_hard "$sig" || return 1
    check "machine responsive after SIG$sig" probe_responsive
}

scenario_crash-sigkill() { scenario_crash KILL; }
scenario_crash-sigsegv() { scenario_crash SEGV; }

scenario_status-deleted() {
    start_healthy interactive || return 1
    rm -f "$STATUS_FILE" "$STATS_FILE"
    log "deleted $STATUS_FILE and $STATS_FILE"
    check "scheduler.json recreated within 3s" wait_for 3 status_running_as interactive
    check "scheduler-stats.json recreated within 3s" wait_for 3 stats_running_as interactive
    check "scheduler kept running (sched_ext enabled)" state_is enabled
    stop_clean TERM
}

scenario_status-corrupt() {
    start_healthy performance || return 1
    printf '{ "running": tru' >"$STATUS_FILE"
    printf 'not json at all' >"$STATS_FILE"
    log "overwrote both files with invalid JSON"
    check "scheduler.json valid again within 3s" wait_for 3 status_running_as performance
    check "scheduler-stats.json valid again within 3s" wait_for 3 stats_running_as performance
    check "scheduler kept running (sched_ext enabled)" state_is enabled
    stop_clean TERM
}

scenario_heartbeat-freeze() {
    start_healthy balanced || return 1
    sleep 1
    local pid="$SCX_PID" hb_before
    hb_before=$(jq .heartbeat "$STATUS_FILE")

    kill -STOP "$pid"
    log "scheduler $pid frozen; the kernel watchdog (timeout_ms = 5000) should eject it"
    log "(this script may itself pause for a few seconds until it does)"
    check "kernel watchdog ejected the frozen scheduler" wait_for 20 state_is disabled
    check "machine responsive once ejected" probe_responsive
    check "heartbeat stopped advancing while frozen" \
        json_ok --argjson b "$hb_before" '(.heartbeat - $b) <= 1' "$STATUS_FILE"
    check "kernel log names the stall" kernel_says "runnable task stall"

    kill -CONT "$pid"
    check "unfrozen scheduler notices and exits by itself" wait_for 10 pid_gone "$pid" || return 1
    reap_scheduler
    check "exit code non-zero: kernel-forced exit reported as an error (got $SCX_RC)" \
        test "$SCX_RC" -ne 0
    check "scheduler.json says stopped" status_stopped
}

scenario_dispatch-errors() {
    start_healthy balanced CERYNTH_SCX_INJECT_DISPATCH_ERRORS=50 || return 1
    sleep 5
    check "still healthy after 5s of failing dispatches" status_running_as balanced
    check "dispatch_errors counted" json_ok '.dispatch_errors > 0' "$STATS_FILE"
    check "work still getting through (tasks_dispatched > 0)" stats_dispatching
    check "failed tasks are retried, not lost (tasks_pending small)" \
        json_ok '.tasks_pending <= 128' "$STATS_FILE"
    check "errors summarized in the scheduler log" grep -q "injected fault" "$SCHED_LOG"
    check "machine responsive" probe_responsive
    log "stats: $(jq -c '{tasks_dispatched, dispatch_errors, tasks_pending}' "$STATS_FILE")"
    stop_clean TERM
}

scenario_dispatch-stall() {
    local seq_before
    seq_before=$(enable_seq)
    launch balanced CERYNTH_SCX_INJECT_DISPATCH_ERRORS=1
    # Not "wait for sched_ext enabled": while every dispatch fails, this
    # script can't get CPU time either, and the scheduler may load and give
    # up before we look. enable_seq remembers that it loaded.
    check "scheduler loaded" wait_for "$START_TIMEOUT" seq_advanced_past "$seq_before"
    check "scheduler gave up on its own" wait_for 15 pid_gone "$SCX_PID" || return 1
    reap_scheduler
    check "exit code non-zero (got $SCX_RC)" test "$SCX_RC" -ne 0
    check "sched_ext disabled" wait_for "$STOP_TIMEOUT" state_is disabled
    check "log explains why" grep -q "no task could be dispatched" "$SCHED_LOG"
    check "left before the kernel watchdog had to step in" \
        kernel_silent_about "runnable task stall"
    check "scheduler.json says stopped" status_stopped
    check "machine responsive" probe_responsive
}

scenario_sysrq-disable() {
    start_healthy interactive || return 1
    sleep 1
    echo S >/proc/sysrq-trigger
    log "sent SysRq-S (kernel: disable sched_ext, revert all tasks to the default scheduler)"
    check "sched_ext disabled" wait_for "$STOP_TIMEOUT" state_is disabled
    check "scheduler noticed and exited by itself" wait_for 10 pid_gone "$SCX_PID" || return 1
    reap_scheduler
    check "exit code non-zero: kernel-forced exit reported as an error (got $SCX_RC)" \
        test "$SCX_RC" -ne 0
    check "scheduler.json says stopped" status_stopped
    check "scheduler-stats.json says stopped" stats_stopped
    check "machine responsive" probe_responsive
}

# A fault only counts as survived if the scheduler can be brought up again
# afterwards and does real work.
check_recovery() {
    if ! reset_to_baseline; then
        fail "recovery: sched_ext stuck in '$(scx_state)'"
        return 1
    fi
    start_healthy balanced || return 1
    sleep 1.5
    check "recovery: fresh scheduler dispatching" stats_dispatching
    stop_clean TERM
}

log "cerynth-scx fault tests: ${SELECTED[*]}"
log "binary: $SCX_BIN, load: $LOAD busy loops"
log "kernel: $(uname -r), sched_ext: $(scx_state)"

RUN_MARK=$(cut -d' ' -f1 /proc/uptime)
start_load "$LOAD"
START_SECONDS=$SECONDS
SCENARIO_RESULTS=()

for s in "${SELECTED[@]}"; do
    log "=== $s: ${DESCRIPTIONS[$s]}"
    fails_before=$FAIL_COUNT
    mark_kernel_log

    "scenario_$s" || log "$s: stopped early after a failed check"
    reset_to_baseline || fail "$s: could not get back to a clean state"

    trouble=$(kernel_trouble_since_mark "${EXPECTED_KERNEL[$s]:-}")
    if [[ -n "$trouble" ]]; then
        fail "$s: unexpected kernel messages"
        printf '%s\n' "$trouble" | head -10 | sed 's/^/      /' | tee -a "$LOG_DIR/progress.log"
    fi

    check_recovery || log "$s: recovery failed"

    if ((FAIL_COUNT == fails_before)); then
        SCENARIO_RESULTS+=("$s=pass")
    else
        SCENARIO_RESULTS+=("$s=fail")
    fi
done

stop_load
DURATION=$((SECONDS - START_SECONDS))
kernel_log_since "$RUN_MARK" >"$LOG_DIR/kernel.log"

SCENARIOS_JSON=$(printf '%s\n' "${SCENARIO_RESULTS[@]}" |
    jq -R 'split("=") | {(.[0]): .[1]}' | jq -s 'add')

write_result fault-test \
    --argjson scenarios "$SCENARIOS_JSON" \
    --argjson load "$LOAD" \
    --argjson duration_s "$DURATION"

log "done in ${DURATION}s: $PASS_COUNT checks passed, $FAIL_COUNT failed"
printf '%s\n' "${SCENARIO_RESULTS[@]}" | sed 's/=/: /; s/^/  /' | tee -a "$LOG_DIR/progress.log"
log "results: $LOG_DIR/result.json"

((FAIL_COUNT == 0))
