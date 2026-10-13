#!/usr/bin/env bash
#
# Shared helpers for scripts/scx/stress-lifecycle.sh and
# scripts/scx/fault-test.sh. Source it, don't run it.
#
# Everything here drives the cerynth-scx binary directly (no cerynthd), the
# same way cerynthd's ScxBackend does: a profile change is a stop followed
# by a start with a different --profile.

# shellcheck disable=SC2034  # variables are used by the sourcing scripts
# shellcheck disable=SC2016  # $names inside jq programs are jq's, not the shell's

SCX_STATE_FILE=/sys/kernel/sched_ext/state
SCX_ENABLE_SEQ_FILE=/sys/kernel/sched_ext/enable_seq
STATUS_FILE=/run/cerynth/scheduler.json
STATS_FILE=/run/cerynth/scheduler-stats.json

PROFILES=(balanced interactive performance background)

# Seconds to wait for a start to become healthy, or a stop to finish.
START_TIMEOUT=${START_TIMEOUT:-5}
STOP_TIMEOUT=${STOP_TIMEOUT:-8}
# A trivial fork+exec taking longer than this means the machine is not
# responsive. Generous on purpose: this catches freezes, not slowness.
RESPONSIVE_LIMIT_MS=${RESPONSIVE_LIMIT_MS:-2000}

SCX_PID=""
LOAD_PIDS=()
PASS_COUNT=0
FAIL_COUNT=0
LOG_DIR=""
SCHED_LOG=""

ts() { date '+%H:%M:%S'; }

log() { printf '%s  %s\n' "$(ts)" "$*" | tee -a "$LOG_DIR/progress.log"; }

pass() {
    PASS_COUNT=$((PASS_COUNT + 1))
    log "PASS  $*"
}

fail() {
    FAIL_COUNT=$((FAIL_COUNT + 1))
    log "FAIL  $*"
}

# check <description> <command...>: run the command, record PASS/FAIL.
check() {
    local description="$1"
    shift
    if "$@"; then
        pass "$description"
    else
        fail "$description"
        return 1
    fi
}

scx_state() { cat "$SCX_STATE_FILE" 2>/dev/null || echo unavailable; }

enable_seq() { cat "$SCX_ENABLE_SEQ_FILE" 2>/dev/null || echo 0; }

# wait_for <timeout seconds> <command...>: poll every 100ms until the command
# succeeds. Uses wall-clock time, so it still works if this script was
# itself starved of CPU for a while.
wait_for() {
    local timeout="$1"
    shift
    local deadline=$(($(date +%s%N) + timeout * 1000000000))
    while (($(date +%s%N) < deadline)); do
        if "$@" 2>/dev/null; then
            return 0
        fi
        sleep 0.1
    done
    "$@" 2>/dev/null
}

state_is() { [[ "$(scx_state)" == "$1" ]]; }

pid_gone() { ! kill -0 "$1" 2>/dev/null; }

json_ok() { jq -e "$@" >/dev/null 2>&1; }

heartbeat_fresh() {
    json_ok --argjson now "$(date +%s)" '($now - .heartbeat) <= 3' "$1"
}

status_running_as() {
    json_ok --arg p "$1" '.running == true and .healthy == true and .profile == $p' "$STATUS_FILE" &&
        heartbeat_fresh "$STATUS_FILE"
}

stats_running_as() {
    json_ok --arg p "$1" '.running == true and .profile == $p and .schema_version == 1' "$STATS_FILE" &&
        heartbeat_fresh "$STATS_FILE"
}

status_stopped() { json_ok '.running == false and .healthy == false' "$STATUS_FILE"; }

stats_stopped() { json_ok '.running == false' "$STATS_FILE"; }

stats_dispatching() { json_ok '.tasks_dispatched > 0' "$STATS_FILE"; }

seq_advanced_past() { (($(enable_seq) > $1)); }

# probe_responsive: time a trivial fork+exec. Fails if it takes longer than
# RESPONSIVE_LIMIT_MS or doesn't finish at all within 10s.
probe_responsive() {
    local start end elapsed_ms
    start=$(date +%s%N)
    timeout 10 /bin/true || return 1
    end=$(date +%s%N)
    elapsed_ms=$(((end - start) / 1000000))
    if ((elapsed_ms > RESPONSIVE_LIMIT_MS)); then
        log "      fork+exec took ${elapsed_ms}ms (limit ${RESPONSIVE_LIMIT_MS}ms)"
        return 1
    fi
}

# start_load <n>: n busy loops, so there is real scheduling work going on.
start_load() {
    local n="$1" i
    for ((i = 0; i < n; i++)); do
        (while :; do :; done) &
        LOAD_PIDS+=("$!")
    done
}

stop_load() {
    local pid
    for pid in "${LOAD_PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    for pid in "${LOAD_PIDS[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    LOAD_PIDS=()
}

# launch <profile> [VAR=value...]: start the scheduler in the background
# without waiting for it. Extra arguments become environment variables.
launch() {
    local profile="$1"
    shift
    printf '\n===== %s launch --profile %s %s\n' "$(ts)" "$profile" "$*" >>"$SCHED_LOG"
    env "$@" "$SCX_BIN" --profile "$profile" >>"$SCHED_LOG" 2>&1 &
    SCX_PID=$!
}

# start_healthy <profile> [VAR=value...]: launch, then require everything
# cerynthd requires before it calls a start successful (live process,
# sched_ext enabled, fresh heartbeat) plus our own files agreeing on the
# profile.
start_healthy() {
    local profile="$1"
    shift
    local seq_before
    seq_before=$(enable_seq)
    launch "$profile" "$@"

    check "start $profile: sched_ext enabled" wait_for "$START_TIMEOUT" state_is enabled || return 1
    check "start $profile: enable_seq advanced (a new scheduler really loaded)" \
        seq_advanced_past "$seq_before" || return 1
    check "start $profile: process alive" kill -0 "$SCX_PID" || return 1
    check "start $profile: scheduler.json running, healthy, fresh" \
        wait_for "$START_TIMEOUT" status_running_as "$profile" || return 1
    check "start $profile: scheduler-stats.json running, fresh" \
        wait_for "$START_TIMEOUT" stats_running_as "$profile" || return 1
}

# stop_clean <signal>: SIGTERM/SIGINT path. Requires exit code 0, kernel
# back on its default scheduler, and both files saying stopped.
stop_clean() {
    local sig="$1" rc=0
    kill "-$sig" "$SCX_PID" 2>/dev/null || true
    check "stop ($sig): process exited" wait_for "$STOP_TIMEOUT" pid_gone "$SCX_PID" || return 1
    wait "$SCX_PID" 2>/dev/null || rc=$?
    SCX_PID=""
    check "stop ($sig): exit code 0 (got $rc)" test "$rc" -eq 0
    check "stop ($sig): sched_ext disabled" wait_for "$STOP_TIMEOUT" state_is disabled || return 1
    check "stop ($sig): scheduler.json says stopped" status_stopped
    check "stop ($sig): scheduler-stats.json says stopped" stats_stopped
}

# kill_hard <signal>: the process gets no chance to clean up. The kernel must
# notice on its own and fall back. The status files are expected to be stale
# afterwards - that is what the heartbeat is for.
kill_hard() {
    local sig="$1"
    kill "-$sig" "$SCX_PID" 2>/dev/null || true
    check "kill ($sig): process gone" wait_for "$STOP_TIMEOUT" pid_gone "$SCX_PID" || return 1
    wait "$SCX_PID" 2>/dev/null || true
    SCX_PID=""
    check "kill ($sig): kernel fell back, sched_ext disabled" \
        wait_for "$STOP_TIMEOUT" state_is disabled || return 1
}

# reset_to_baseline: whatever happened, get back to "no scheduler loaded".
# Escalates SIGTERM -> SIGKILL -> SysRq-S (kernel-side "disable sched_ext and
# revert all tasks to the default scheduler"). Returns non-zero only if even
# that fails, i.e. the machine is in a state we can't recover from.
reset_to_baseline() {
    if [[ -n "$SCX_PID" ]] && kill -0 "$SCX_PID" 2>/dev/null; then
        kill -CONT "$SCX_PID" 2>/dev/null || true
        kill -TERM "$SCX_PID" 2>/dev/null || true
        wait_for "$STOP_TIMEOUT" pid_gone "$SCX_PID" || kill -KILL "$SCX_PID" 2>/dev/null || true
        wait_for "$STOP_TIMEOUT" pid_gone "$SCX_PID" || true
        wait "$SCX_PID" 2>/dev/null || true
    fi
    SCX_PID=""
    if wait_for "$STOP_TIMEOUT" state_is disabled; then
        return 0
    fi
    log "WARN  sched_ext still '$(scx_state)' after the scheduler is gone; trying SysRq-S"
    echo S >/proc/sysrq-trigger 2>/dev/null || true
    wait_for "$STOP_TIMEOUT" state_is disabled
}

# Kernel log lines that mean something went wrong that a test did not ask
# for. Expected messages (a scheduler unregistering, SysRq when the test
# triggered it) don't match. Unrelated driver warnings don't either, so a
# noisy host doesn't fail the run.
KERNEL_TROUBLE='runnable task stall|scx_bpf_error|BUG:|Oops|soft lockup|hard LOCKUP|blocked for more than|rcu.*stall|WARNING:.*sched/ext'

# Kernel log positions are tracked by timestamp (seconds since boot, same
# clock as dmesg's [12345.678901] prefix) rather than line count, so a
# ring buffer that wraps during a long run doesn't skew them.
DMESG_MARK=0

mark_kernel_log() { DMESG_MARK=$(cut -d' ' -f1 /proc/uptime); }

kernel_log_since() {
    dmesg 2>/dev/null | awk -F'[][]' -v t="$1" '($2 + 0) >= t'
}

kernel_log_since_mark() { kernel_log_since "$DMESG_MARK"; }

# kernel_trouble_since_mark [allowed regex]: trouble lines since the mark,
# minus ones the current test expects (e.g. a stall it caused on purpose).
kernel_trouble_since_mark() {
    local allowed="${1:-}"
    if [[ -n "$allowed" ]]; then
        kernel_log_since_mark | grep -Ei "$KERNEL_TROUBLE" | grep -Eiv "$allowed"
    else
        kernel_log_since_mark | grep -Ei "$KERNEL_TROUBLE"
    fi
}

# require_root_and_tools <script args...>: re-exec under sudo if needed.
require_root_and_tools() {
    if ((EUID != 0)); then
        echo "Needs root (loads a BPF scheduler). Re-running with sudo..." >&2
        exec sudo --preserve-env=START_TIMEOUT,STOP_TIMEOUT,RESPONSIVE_LIMIT_MS "$0" "$@"
    fi
    local tool
    for tool in jq dmesg timeout; do
        if ! command -v "$tool" >/dev/null; then
            echo "ERROR: '$tool' not found (Ubuntu: sudo apt install -y $tool)" >&2
            exit 2
        fi
    done
    if [[ ! -r "$SCX_STATE_FILE" ]]; then
        echo "ERROR: $SCX_STATE_FILE missing - this kernel has no sched_ext." >&2
        exit 2
    fi
}

# default_binary <repo root>: freshly built binary if there is one, otherwise
# the installed one.
default_binary() {
    if [[ -x "$1/target/release/cerynth-scx" ]]; then
        echo "$1/target/release/cerynth-scx"
    else
        echo /usr/lib/cerynth/cerynth-scx
    fi
}

preflight() {
    if [[ ! -x "$SCX_BIN" ]]; then
        echo "ERROR: scheduler binary not found: $SCX_BIN" >&2
        echo "       build it first: cargo build --release -p cerynth-scx" >&2
        exit 2
    fi
    if ! state_is disabled; then
        echo "ERROR: sched_ext is '$(scx_state)' - another scheduler is already loaded." >&2
        echo "       If cerynthd started it: sudo cerynthctl stop" >&2
        echo "       Otherwise: ./scripts/scx/stop-scheduler.sh" >&2
        exit 2
    fi
    if command -v systemctl >/dev/null && systemctl is-active --quiet cerynthd 2>/dev/null; then
        echo "WARN: cerynthd is running. Don't start a scheduler through it during this test." >&2
    fi
}

# Hand the log directory back to whoever ran sudo, so it isn't root-owned
# inside their checkout.
chown_log_dir() {
    if [[ -n "${SUDO_UID:-}" && -n "${SUDO_GID:-}" ]]; then
        chown -R "$SUDO_UID:$SUDO_GID" "$LOG_DIR" 2>/dev/null || true
    fi
}

setup_log_dir() {
    LOG_DIR="$1"
    mkdir -p "$LOG_DIR"
    SCHED_LOG="$LOG_DIR/scheduler.log"
    : >"$LOG_DIR/progress.log"
    : >"$SCHED_LOG"
}

# write_result <name> [--arg/--argjson key value...]: result.json summarizing
# the run. Extra named arguments become extra fields.
write_result() {
    local name="$1"
    shift
    jq -n \
        --arg test "$name" \
        --arg kernel "$(uname -r)" \
        --arg binary "$SCX_BIN" \
        --argjson checks_passed "$PASS_COUNT" \
        --argjson checks_failed "$FAIL_COUNT" \
        "$@" \
        '$ARGS.named + {result: (if $ARGS.named.checks_failed == 0 then "pass" else "fail" end)}' \
        >"$LOG_DIR/result.json"
}
