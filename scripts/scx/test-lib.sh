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
# Seconds to wait for the kernel to take over after the scheduler is killed
# outright. Longer than the kernel watchdog's worst case (~10s), so a slow
# fallback is measured and reported instead of being cut off as "never".
HARD_KILL_TIMEOUT=${HARD_KILL_TIMEOUT:-20}
# How long that fallback may take. A healthy fallback takes a few hundred
# ms; 5000ms or more means the process could not finish dying and the kernel
# watchdog had to eject the scheduler, with the machine stalled meanwhile.
FALLBACK_LIMIT_MS=${FALLBACK_LIMIT_MS:-3000}
# A trivial fork+exec taking longer than this means the machine is not
# responsive. Generous on purpose: this catches freezes, not slowness.
RESPONSIVE_LIMIT_MS=${RESPONSIVE_LIMIT_MS:-2000}
# Exit code cerynth-scx uses for "this start failed, but starting again is
# expected to work" (EXIT_START_AGAIN in src/main.rs). The scheduler cannot
# load a second time in the same process, so whoever started it has to start
# it again. In these tests that is start_healthy.
EXIT_START_AGAIN=75
# How many launches one start may take before it counts as failed.
START_ATTEMPTS=${START_ATTEMPTS:-3}

SCX_PID=""
LOAD_PIDS=()
PASS_COUNT=0
FAIL_COUNT=0
LOG_DIR=""
SCHED_LOG=""
# Every hard-kill fallback time measured in this run, in ms.
FALLBACK_MS=()
# Launches in this run that ended with EXIT_START_AGAIN and were repeated.
START_AGAIN_COUNT=0
# Extra environment for the first launch of the next start_healthy only, so
# a test can make exactly one load fail.
FIRST_LAUNCH_ENV=()

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

# The status file was written by the scheduler we launched, not left behind
# by an earlier one.
status_is_ours() { json_ok --argjson pid "$SCX_PID" '.pid == $pid' "$STATUS_FILE"; }

status_stopped() { json_ok '.running == false and .healthy == false' "$STATUS_FILE"; }

stats_stopped() { json_ok '.running == false' "$STATS_FILE"; }

stats_dispatching() { json_ok '.tasks_dispatched > 0' "$STATS_FILE"; }

seq_advanced_past() { (($(enable_seq) > $1)); }

# up_or_gone <profile> <enable_seq before the launch>: the launched scheduler
# has either come up completely or exited.
up_or_gone() {
    pid_gone "$SCX_PID" && return 0
    state_is enabled && seq_advanced_past "$2" && status_is_ours && status_running_as "$1"
}

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
#
# A launch that exits with EXIT_START_AGAIN is repeated, up to START_ATTEMPTS
# launches in total, the way a supervisor is expected to. Each repeat is
# counted in START_AGAIN_COUNT so it shows up in the results.
start_healthy() {
    local profile="$1"
    shift
    local seq_before attempt rc=0
    local first_env=("${FIRST_LAUNCH_ENV[@]}")
    FIRST_LAUNCH_ENV=()

    for ((attempt = 1; attempt <= START_ATTEMPTS; attempt++)); do
        # Taken before every launch: a failed load may have advanced it too.
        seq_before=$(enable_seq)
        if ((attempt == 1)); then
            launch "$profile" "$@" "${first_env[@]}"
        else
            launch "$profile" "$@"
        fi
        # Neither up nor gone in time: let the checks below say what's wrong.
        wait_for "$START_TIMEOUT" up_or_gone "$profile" "$seq_before" || break
        pid_gone "$SCX_PID" || break
        rc=0
        wait "$SCX_PID" 2>/dev/null || rc=$?
        SCX_PID=""
        ((rc == EXIT_START_AGAIN)) || break
        START_AGAIN_COUNT=$((START_AGAIN_COUNT + 1))
        log "      load failed, scheduler asked to be started again" \
            "(exit code $rc, launch $attempt of $START_ATTEMPTS)"
    done
    if [[ -z "$SCX_PID" ]]; then
        fail "start $profile: scheduler exited with code $rc instead of coming up"
        return 1
    fi

    check "start $profile: sched_ext enabled" wait_for "$START_TIMEOUT" state_is enabled || return 1
    check "start $profile: enable_seq advanced (a new scheduler really loaded)" \
        seq_advanced_past "$seq_before" || return 1
    check "start $profile: process alive" kill -0 "$SCX_PID" || return 1
    check "start $profile: scheduler.json running, healthy, fresh" \
        wait_for "$START_TIMEOUT" status_running_as "$profile" || return 1
    check "start $profile: scheduler-stats.json running, fresh" \
        wait_for "$START_TIMEOUT" stats_running_as "$profile" || return 1

    # Once per run: prove the kernel-log checks can actually see something.
    # Every load prints a "BPF scheduler ... enabled" line; if we can't find
    # it, a later "kernel log clean" would be a pass that looked at nothing.
    if [[ "$KLOG_VERIFIED" != true ]]; then
        check "kernel log readable (sees this scheduler being enabled)" \
            kernel_says 'BPF scheduler .* enabled' || return 1
        KLOG_VERIFIED=true
    fi
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
# notice on its own and fall back, and do it quickly. The status files are
# expected to be stale afterwards - that is what the heartbeat is for.
#
# The time from the signal to "sched_ext disabled" is measured as this script
# experiences it, which is the point: while a dead scheduler is still loaded,
# ordinary processes like this one don't get CPU time.
kill_hard() {
    local sig="$1" start elapsed_ms
    start=$(date +%s%N)
    kill "-$sig" "$SCX_PID" 2>/dev/null || true
    check "kill ($sig): process gone" wait_for "$HARD_KILL_TIMEOUT" pid_gone "$SCX_PID" || return 1
    wait "$SCX_PID" 2>/dev/null || true
    SCX_PID=""
    check "kill ($sig): kernel fell back, sched_ext disabled" \
        wait_for "$HARD_KILL_TIMEOUT" state_is disabled || return 1
    elapsed_ms=$((($(date +%s%N) - start) / 1000000))
    FALLBACK_MS+=("$elapsed_ms")
    # A slow fallback is recorded as a failed check, but the scheduler *is*
    # gone, so the caller can carry on with its next step.
    check "kill ($sig): fallback took ${elapsed_ms}ms (limit ${FALLBACK_LIMIT_MS}ms)" \
        test "$elapsed_ms" -le "$FALLBACK_LIMIT_MS"
    return 0
}

# fallback_stats: "count max avg" over every fallback measured so far.
fallback_stats() {
    local ms sum=0 max=0 n=${#FALLBACK_MS[@]}
    for ms in "${FALLBACK_MS[@]}"; do
        sum=$((sum + ms))
        ((ms > max)) && max=$ms
    done
    if ((n == 0)); then
        echo "0 0 0"
    else
        echo "$n $max $((sum / n))"
    fi
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

# Reading the kernel log "since this test step started".
#
# The position is a marker line this script writes into the kernel log
# itself, and "since" means "everything after that line". No clocks are
# involved on purpose: the first version compared /proc/uptime against the
# [seconds] stamps in dmesg, but those are two different clocks (uptime keeps
# counting while a laptop is suspended, the kernel log's clock does not). On
# a machine that had slept for ten hours every line looked older than the
# mark, the log came back empty, and "kernel log clean" passed having read
# nothing - while the kernel was reporting 84 watchdog stalls.
KLOG_MARK=""
KLOG_SEQ=0
KLOG_TEXT=""
KLOG_VERIFIED=false

mark_kernel_log() {
    KLOG_SEQ=$((KLOG_SEQ + 1))
    KLOG_MARK="cerynth-test-mark:$$:$KLOG_SEQ:"
    echo "$KLOG_MARK" >/dev/kmsg 2>/dev/null || KLOG_MARK=""
}

# read_kernel_log: put the kernel messages since the mark into KLOG_TEXT.
# Returns non-zero if the mark can't be found (never written, or pushed out
# of the kernel's ring buffer); KLOG_TEXT then holds the whole buffer, so
# callers over-report rather than miss something.
read_kernel_log() {
    local all
    KLOG_TEXT=""
    all=$(dmesg 2>/dev/null) || return 1
    if [[ -n "$KLOG_MARK" && "$all" == *"$KLOG_MARK"* ]]; then
        KLOG_TEXT=${all##*"$KLOG_MARK"}
        # Drop what is left of the marker's own line.
        if [[ "$KLOG_TEXT" == *$'\n'* ]]; then
            KLOG_TEXT=${KLOG_TEXT#*$'\n'}
        else
            KLOG_TEXT=""
        fi
        return 0
    fi
    KLOG_TEXT=$all
    return 1
}

# kernel_says <regex>: does the kernel log since the mark contain this?
kernel_says() {
    read_kernel_log
    grep -Eq "$1" <<<"$KLOG_TEXT"
}

# check_kernel_log <label> [allowed regex]: one PASS/FAIL for "nothing went
# wrong in the kernel since the mark", and append what was read to
# kernel.log. Lines matching the allowed regex are trouble a test caused on
# purpose. Fails, rather than passes, if the log could not be read.
check_kernel_log() {
    local label="$1" allowed="${2:-}" trouble
    if ! read_kernel_log; then
        fail "$label: kernel log could not be read from this step's marker"
        return 1
    fi
    printf '%s\n' "$KLOG_TEXT" >>"$LOG_DIR/kernel.log"

    trouble=$(grep -Ei "$KERNEL_TROUBLE" <<<"$KLOG_TEXT")
    if [[ -n "$allowed" && -n "$trouble" ]]; then
        trouble=$(grep -Eiv "$allowed" <<<"$trouble")
    fi
    if [[ -n "$trouble" ]]; then
        fail "$label: unexpected kernel messages"
        printf '%s\n' "$trouble" | head -10 | sed 's/^/      /' | tee -a "$LOG_DIR/progress.log"
        return 1
    fi
    pass "$label: kernel log clean"
}

# require_root_and_tools <script args...>: re-exec under sudo if needed.
require_root_and_tools() {
    if ((EUID != 0)); then
        echo "Needs root (loads a BPF scheduler). Re-running with sudo..." >&2
        exec sudo --preserve-env=START_TIMEOUT,STOP_TIMEOUT,HARD_KILL_TIMEOUT,FALLBACK_LIMIT_MS,RESPONSIVE_LIMIT_MS "$0" "$@"
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
    : >"$LOG_DIR/kernel.log"
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
