#!/usr/bin/env bash
#
# CerynthOS VM integration smoke test.
#
# Verifies that a provisioned VM boots the custom kernel with sched_ext and
# BTF available, that cerynthd is running under systemd, and that the IPC
# surface described in docs/contracts/runtime-v1.md actually exists.
#
#   ./scripts/vm/smoke-test.sh
#
# Artifacts land in artifacts/smoke-test/.

set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# shellcheck source=scripts/vm/cerynth-vm-common.sh
source "$ROOT_DIR/scripts/vm/cerynth-vm-common.sh"

ARTIFACT_DIR="${CERYNTH_SMOKE_ARTIFACTS:-$ROOT_DIR/artifacts/smoke-test}"
mkdir -p "$ARTIFACT_DIR"

PASS_COUNT=0
FAIL_COUNT=0

# Parallel arrays holding each check's label and outcome, used to build
# result.json and the summary table.
CHECK_LABELS=()
CHECK_RESULTS=()

record() {
    CHECK_LABELS+=("$1")
    CHECK_RESULTS+=("$2")
}

pass() {
    printf '[PASS] %s\n' "$2"
    record "$1" PASS
    PASS_COUNT=$((PASS_COUNT + 1))
}

fail() {
    printf '[FAIL] %s\n' "$2"
    record "$1" FAIL
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

# check <label> <description> <remote command>
check() {
    local label="$1" description="$2"
    shift 2

    if vm_ssh "$@" >/dev/null 2>&1; then
        pass "$label" "$description"
    else
        fail "$label" "$description"
    fi
}

printf 'CerynthOS VM smoke test\n'
printf '=======================\n\n'

require_vm_key

if vm_ssh true 2>/dev/null; then
    pass "ssh" "SSH connection"
else
    fail "ssh" "SSH connection"
    printf '\nCannot reach %s port %s. Is the VM running?\n' "$VM_TARGET" "$VM_PORT" >&2
    printf '  ./scripts/vm/run-cerynth-vm.sh\n' >&2
    exit 1
fi

KERNEL="$(vm_ssh 'uname -r' 2>/dev/null || true)"

if [[ -n "$KERNEL" ]]; then
    pass "kernel" "Kernel running: $KERNEL"
else
    fail "kernel" "Kernel: uname -r produced no output"
fi

check btf "BTF at /sys/kernel/btf/vmlinux" 'test -r /sys/kernel/btf/vmlinux'

SCHED_EXT="$(vm_ssh 'cat /sys/kernel/sched_ext/state 2>/dev/null' 2>/dev/null || true)"
SCHED_EXT="${SCHED_EXT//[$'\r\n']/}"

# Either state is a pass: "disabled" simply means no BPF scheduler is
# attached right now, which is the correct idle state.
case "$SCHED_EXT" in
    enabled|disabled)
        pass "sched_ext" "sched_ext available: $SCHED_EXT"
        ;;
    *)
        fail "sched_ext" "sched_ext unavailable (/sys/kernel/sched_ext/state unreadable)"
        ;;
esac

check cerynthd    "cerynthd active"        'systemctl is-active --quiet cerynthd'
check socket      "IPC socket at /run/cerynth/cerynthd.sock" \
                                           'test -S /run/cerynth/cerynthd.sock'

# The control socket is mode 0660 and owned by root: talking to the control
# plane is a privileged operation, so the CLI is exercised through sudo.
check cerynthctl  "cerynthctl status"      'sudo -n cerynthctl status'

# Checks the recorded kernel matches the running one, not just that the file
# exists: a marker whose contents are wrong is worse than no marker at all.
if [[ -n "$KERNEL" ]] && \
   vm_ssh "grep -qx 'kernel=$KERNEL' /run/cerynth/boot-ok" >/dev/null 2>&1; then
    pass "boot_marker" "Boot marker records kernel $KERNEL"
elif vm_ssh 'test -e /run/cerynth/boot-ok' >/dev/null 2>&1; then
    fail "boot_marker" "Boot marker exists but does not record kernel $KERNEL"
else
    fail "boot_marker" "Boot marker missing at /run/cerynth/boot-ok"
fi

printf '\nCollecting artifacts...\n'

# shellcheck disable=SC2119  # this remote script takes no arguments
vm_bash >"$ARTIFACT_DIR/system-info.txt" 2>&1 <<'REMOTE' || true
echo "=== uname ==="
uname -a

echo
echo "=== kernel command line ==="
cat /proc/cmdline

echo
echo "=== system state ==="
systemctl is-system-running || true

echo
echo "=== cerynthd ==="
systemctl --no-pager --full status cerynthd || true

echo
echo "=== runtime directory ==="
ls -la /run/cerynth 2>/dev/null || echo "missing"

echo
echo "=== boot marker ==="
cat /run/cerynth/boot-ok 2>/dev/null || echo "missing"

echo
echo "=== configuration ==="
cat /etc/cerynth/cerynth.toml 2>/dev/null || echo "missing"

echo
echo "=== sched_ext ==="
cat /sys/kernel/sched_ext/state 2>/dev/null || echo "unavailable"

echo
echo "=== scheduler status ==="
cat /run/cerynth/scheduler.json 2>/dev/null || echo "not present"

echo
echo "=== cerynthctl status ==="
sudo -n cerynthctl status 2>&1 || true

echo
echo "=== failed units ==="
systemctl --failed --no-pager || true
REMOTE

vm_ssh 'sudo -n journalctl -u cerynthd -b --no-pager' \
    >"$ARTIFACT_DIR/journal.log" 2>&1 || true

vm_ssh 'sudo -n dmesg' \
    >"$ARTIFACT_DIR/dmesg.log" 2>&1 || true

# result.json, assembled field by field so the checks array stays valid JSON.
{
    printf '{\n'
    printf '  "success": %s,\n' "$([[ "$FAIL_COUNT" -eq 0 ]] && echo true || echo false)"
    printf '  "passed": %d,\n' "$PASS_COUNT"
    printf '  "failed": %d,\n' "$FAIL_COUNT"
    printf '  "kernel": "%s",\n' "$KERNEL"
    printf '  "sched_ext_state": "%s",\n' "${SCHED_EXT:-unavailable}"
    printf '  "checks": {\n'

    for i in "${!CHECK_LABELS[@]}"; do
        local_sep=","
        [[ "$i" -eq $(( ${#CHECK_LABELS[@]} - 1 )) ]] && local_sep=""
        printf '    "%s": "%s"%s\n' \
            "${CHECK_LABELS[$i]}" "${CHECK_RESULTS[$i]}" "$local_sep"
    done

    printf '  }\n'
    printf '}\n'
} >"$ARTIFACT_DIR/result.json"

printf '\nSummary\n-------\n'
for i in "${!CHECK_LABELS[@]}"; do
    printf '  %-14s %s\n' "${CHECK_LABELS[$i]}:" "${CHECK_RESULTS[$i]}"
done

printf '\nPassed: %d\n' "$PASS_COUNT"
printf 'Failed: %d\n' "$FAIL_COUNT"
printf 'Artifacts: %s\n' "$ARTIFACT_DIR"

if [[ "$FAIL_COUNT" -eq 0 ]]; then
    printf '\nSMOKE TEST PASSED\n'
    exit 0
fi

printf '\nSMOKE TEST FAILED\n'
printf 'Start debugging with: %s/journal.log\n' "$ARTIFACT_DIR"
exit 1
