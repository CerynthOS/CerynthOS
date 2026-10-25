#!/usr/bin/env bash
# Reviewed transaction driver: run once with sudo in the Ubuntu VM terminal.
# Builds/workloads run unprivileged; no privileged actions occur when sourced.
set -euo pipefail

sudo_rule() {
  printf '%s ALL=(root) NOPASSWD: /usr/bin/cerynthctl status, /usr/bin/cerynthctl start, /usr/bin/cerynthctl stop, /usr/bin/cerynthctl profile set balanced, /usr/bin/cerynthctl profile set interactive, /usr/bin/cerynthctl profile set performance, /usr/bin/cerynthctl profile set background\n' "$RUN_USER"
}

ctl() {
  local response status=0
  response=$(timeout 30 /usr/bin/cerynthctl "$@" 2>&1) || status=$?
  printf '%s\n' "$response"
  [[ "$status" -eq 0 && "$response" != *'✗'* ]]
}

healthy() {
  local daemon_pid
  daemon_pid=$(systemctl show cerynthd -p MainPID --value) || return 1
  python3 - "$daemon_pid" <<'PY'
import json, os, sys, time
from pathlib import Path
s=json.loads(Path('/run/cerynth/scheduler.json').read_text())
assert s['running'] is True and s['healthy'] is True
assert 0 <= time.time()-s['heartbeat'] <= 5
assert Path('/sys/kernel/sched_ext/state').read_text().strip() == 'enabled'
assert Path('/proc/sys/vm/compact_unevictable_allowed').read_text().strip() == '0'
os.kill(s['pid'], 0)
assert Path(f"/proc/{s['pid']}/exe").resolve() == Path('/usr/lib/cerynth/cerynth-scx')
parent=[row for row in Path(f"/proc/{s['pid']}/status").read_text().splitlines() if row.startswith('PPid:')][0]
assert int(parent.split()[1]) == int(sys.argv[1]) > 0
assert s['profile'].lower() == 'balanced'
PY
}

wait_healthy() {
  local attempt
  for attempt in $(seq 1 20); do
    if healthy >> "$RESULT/health-checks.log" 2>&1; then return 0; fi
    sleep 1
  done
  return 1
}

# Stop the whole systemd cgroup, including workloads that created new sessions.
stop_validation_units() {
  local unit cg path attempt load failed=0
  if [[ -n "$LAUNCHER_PID" ]]; then
    kill -TERM "$LAUNCHER_PID" 2>/dev/null || true
    wait "$LAUNCHER_PID" 2>/dev/null || true
    LAUNCHER_PID=''
  fi
  for unit in "${UNITS[@]}"; do
    load=$(systemctl show "$unit" -p LoadState --value 2>/dev/null) || load=unknown
    cg=$(systemctl show "$unit" -p ControlGroup --value 2>/dev/null) || cg=''
    [[ "$load" != unknown ]] || failed=1
    printf 'Draining unit=%s cgroup=%s load=%s\n' "$unit" "$cg" "$load" >> "$RESULT/workload-cleanup.log"
    timeout 25 systemctl stop "$unit" >> "$RESULT/workload-cleanup.log" 2>&1 || true
    if [[ -n "$cg" ]]; then
      if [[ "$cg" != "/system.slice/$unit" ]]; then
        printf 'Unexpected cgroup for %s: %s\n' "$unit" "$cg" >> "$RESULT/workload-cleanup.log"
        failed=1
        continue
      fi
      path="/sys/fs/cgroup$cg"
      if [[ -e "$path/cgroup.events" ]] && grep -q '^populated 1$' "$path/cgroup.events"; then
        systemctl kill --kill-whom=all --signal=SIGKILL "$unit" >> "$RESULT/workload-cleanup.log" 2>&1 || true
        if [[ -w "$path/cgroup.kill" ]]; then printf '1\n' > "$path/cgroup.kill" || true; fi
      fi
      for attempt in $(seq 1 10); do
        [[ -e "$path/cgroup.events" ]] || break
        grep -q '^populated 1$' "$path/cgroup.events" || break
        sleep 1
      done
      if [[ -e "$path/cgroup.events" ]] && grep -q '^populated 1$' "$path/cgroup.events"; then failed=1; fi
    fi
    if systemctl is-active --quiet "$unit"; then failed=1; fi
  done
  printf 'Workload cgroup cleanup result: %s\n' "$failed" >> "$RESULT/workload-cleanup.log"
  return "$failed"
}

linux_fallback() {
  timeout 30 systemctl stop cerynth-policy-live cerynthd || true
  if [[ "$(cat /sys/kernel/sched_ext/state)" != disabled ]]; then
    systemctl kill --kill-whom=all --signal=SIGKILL cerynthd || true
  fi
  local attempt
  for attempt in $(seq 1 20); do
    [[ "$(cat /sys/kernel/sched_ext/state)" == disabled ]] && return 0
    sleep .1
  done
  return 1
}

restore_previous_runtime() {
  timeout 30 systemctl stop cerynth-policy-live cerynthd || return 1
  cp -p "$BACKUP/cerynthd" "$RESTORE_TEMP" || return 1
  mv "$RESTORE_TEMP" /usr/lib/cerynth/cerynthd || return 1
  cp -p "$BACKUP/cerynth-scx" "$RESTORE_SCX_TEMP" || return 1
  mv "$RESTORE_SCX_TEMP" /usr/lib/cerynth/cerynth-scx || return 1
  cp -p "$BACKUP/cerynth.toml" /etc/cerynth/cerynth.toml || return 1
  if [[ "$STATE_EXISTED" -eq 1 ]]; then
    cp -p "$BACKUP/state.json" /var/lib/cerynth/state.json || return 1
  else
    rm -f /var/lib/cerynth/state.json || return 1
  fi
  cmp -s "$BACKUP/cerynthd" /usr/lib/cerynth/cerynthd || return 1
  cmp -s "$BACKUP/cerynth-scx" /usr/lib/cerynth/cerynth-scx || return 1
  cmp -s "$BACKUP/cerynth.toml" /etc/cerynth/cerynth.toml || return 1
  [[ "$(cat /sys/kernel/sched_ext/state)" == disabled ]]
}

capture_failure() {
  mkdir -p "$RESULT/failure-evidence"
  journalctl --after-cursor "$JOURNAL_CURSOR" -u cerynthd --no-pager -o json > "$RESULT/failure-evidence/daemon.jsonl" 2>&1 || true
  journalctl --after-cursor "$JOURNAL_CURSOR" -k --no-pager -o json > "$RESULT/failure-evidence/kernel.jsonl" 2>&1 || true
  for name in scheduler.json scheduler-stats.json policy.json; do
    [[ ! -f "/run/cerynth/$name" ]] || cp -p "/run/cerynth/$name" "$RESULT/failure-evidence/$name"
  done
  cat /sys/kernel/sched_ext/state > "$RESULT/failure-evidence/kernel-state.txt"
}

create_control_rule() {
  RULE_TEMP=$(mktemp /etc/sudoers.d/.cerynth-validation.XXXXXX)
  sudo_rule > "$RULE_TEMP"
  chmod 0440 "$RULE_TEMP"
  visudo -cf "$RULE_TEMP"
  RULE_CREATED=1
  mv -n "$RULE_TEMP" "$RULE"
  [[ ! -e "$RULE_TEMP" ]] || { RULE_CREATED=0; return 1; }
  RULE_TEMP=''
}

remove_control_rule() {
  if [[ "$RULE_CREATED" -eq 1 ]]; then rm -f "$RULE"; RULE_CREATED=0; fi
  [[ ! -e "$RULE" && ! -L "$RULE" ]] || return 1
  printf 'temporary-rule-absent: verified\n'
  visudo -c
}

stop_sampler() {
  if [[ -n "${SAMPLER_PID:-}" ]]; then
    kill -TERM "$SAMPLER_PID" 2>/dev/null || true
    wait "$SAMPLER_PID" 2>/dev/null || true
    SAMPLER_PID=''
  fi
}

start_sampler() {
  python3 "$SOURCE/tools/sample-scheduler-stacks.py" --output "$RESULT/blocked-stacks.jsonl" \
    --ready "$RESULT/stack-sampler-ready.json" --parent-pid "$$" \
    --daemon-pid "$(systemctl show cerynthd -p MainPID --value)" > "$RESULT/stack-sampler.log" 2>&1 &
  SAMPLER_PID=$!
  local attempt
  for attempt in $(seq 1 40); do
    if [[ -f "$RESULT/stack-sampler-ready.json" ]]; then return 0; fi
    kill -0 "$SAMPLER_PID" 2>/dev/null || return 1
    sleep .1
  done
  return 1
}

restore_allocator_setting() {
  local path=$1 expected=$2
  [[ "$expected" == 0 || "$expected" == 1 ]] || return 1
  if [[ "$(cat "$path")" != "$expected" ]]; then
    printf '%s\n' "$expected" > "$path" || return 1
  fi
  [[ "$(cat "$path")" == "$expected" ]]
}

cleanup() {
  local code=$? workloads=ok recovery=unchanged permissions=absent baseline=unchecked
  trap - EXIT
  trap '' INT TERM HUP
  set +e
  # Preserve the failure before stopping anything; never reattach after failure.
  if [[ "$code" -ne 0 && "$MUTATED" -eq 1 ]]; then capture_failure; fi
  # Always drain workload cgroups BEFORE rollback or privilege revocation.
  stop_validation_units || { workloads=failed; code=1; }
  stop_sampler
  runuser -u "$RUN_USER" -- python3 "$SOURCE/tools/validation_manifest.py" "$OLD_MATRIX" > "$RESULT/baseline-after.json"
  if cmp -s "$RESULT/baseline-before.json" "$RESULT/baseline-after.json"; then baseline=unchanged;
  else baseline=changed; code=1; fi
  if [[ "$MUTATED" -eq 1 ]]; then
    if [[ "$code" -eq 0 && "${DIAGNOSTIC_ONLY:-0}" -eq 0 ]] && healthy >> "$RESULT/health-checks.log" 2>&1; then
      recovery=updated-daemon-balanced
    else
      if [[ "${DIAGNOSTIC_ONLY:-0}" -eq 0 ]]; then [[ "$code" -ne 0 ]] || code=1; fi
      capture_failure
      if linux_fallback; then
        if restore_previous_runtime; then recovery=previous-files-linux-default;
        else recovery=linux-default-restore-failed; code=1; fi
      else recovery=failed; code=1; fi
    fi
  fi
  # The allocator changes this setting itself. Verify its restoration on
  # Linux fallback, or its required value while the updated runtime is active.
  if [[ -f "${BACKUP:-/nonexistent}/compact_unevictable_allowed" ]]; then
    local observed_vm expected_vm
    observed_vm=$(cat /proc/sys/vm/compact_unevictable_allowed)
    expected_vm=$(cat "$BACKUP/compact_unevictable_allowed")
    if [[ "$(cat /sys/kernel/sched_ext/state)" == disabled ]]; then
      restore_allocator_setting /proc/sys/vm/compact_unevictable_allowed "$expected_vm" || code=1
      observed_vm=$(cat /proc/sys/vm/compact_unevictable_allowed)
    else expected_vm=0; fi
    printf 'expected=%s observed=%s\n' "$expected_vm" "$observed_vm" > "$RESULT/allocator-vm-setting-check.txt"
    [[ "$observed_vm" == "$expected_vm" ]] || code=1
  fi
  ctl status > "$RESULT/final-runtime-status.txt" 2>&1
  systemctl is-active cerynthd cerynth-policy-live > "$RESULT/final-services.txt"
  cat /sys/kernel/sched_ext/state > "$RESULT/final-kernel-state.txt"
  if [[ "$RULE_CREATED" -eq 1 ]]; then rm -f "$RULE"; fi
  [[ -z "$RULE_TEMP" ]] || rm -f "$RULE_TEMP"
  if [[ -e "$RULE" || -L "$RULE" ]]; then permissions=present; code=1; fi
  [[ -z "${INSTALL_TEMP:-}" ]] || rm -f "$INSTALL_TEMP"
  [[ -z "${RESTORE_TEMP:-}" ]] || rm -f "$RESTORE_TEMP"
  [[ -z "${INSTALL_SCX_TEMP:-}" ]] || rm -f "$INSTALL_SCX_TEMP"
  [[ -z "${RESTORE_SCX_TEMP:-}" ]] || rm -f "$RESTORE_SCX_TEMP"
  visudo -c > "$RESULT/sudoers-check.txt" 2>&1 || code=1
  python3 - "$RESULT" "$code" "$workloads" "$recovery" "$permissions" "$baseline" <<'PY'
import json, sys
from pathlib import Path
root, code, workloads, recovery, permissions, baseline=sys.argv[1:]
Path(root,'cleanup-result.json').write_text(json.dumps(dict(exit_code=int(code), workloads=workloads,
    recovery=recovery, temporary_rule=permissions, baseline=baseline), indent=2)+'\n')
PY
  chown -hR "$RUN_USER:$(id -gn "$RUN_USER")" "$RESULT"
  printf 'Validation exit: %s; results: %s\n' "$code" "$RESULT"
  exit "$code"
}

phase() {
  local name=$1
  shift
  local unit="${SESSION}-${name}.service" status=0
  case "$name" in smoke|stability|diagnostic|matrix) create_control_rule ;; esac
  UNITS+=("$unit")
  printf '%s\n' "$unit" >> "$RESULT/workload-units.txt"
  systemd-run --unit="$unit" --service-type=exec --wait --pipe --collect --quiet \
    --property="User=$RUN_USER" --property="Group=$(id -gn "$RUN_USER")" \
    --property=KillMode=control-group --property=TimeoutStopSec=10s --property=SendSIGKILL=yes \
    --property="WorkingDirectory=$SOURCE" \
    --setenv="HOME=$RUN_HOME" --setenv="PATH=$RUN_HOME/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    --setenv="CARGO_HOME=$RUN_HOME/.cargo" --setenv="RUSTUP_HOME=$RUN_HOME/.rustup" \
    "$@" > "$RESULT/$name.log" 2>&1 &
  LAUNCHER_PID=$!
  wait "$LAUNCHER_PID" || status=$?
  LAUNCHER_PID=''
  case "$name" in smoke|stability|diagnostic)
    [[ -n "${SAMPLER_PID:-}" ]] && kill -0 "$SAMPLER_PID" 2>/dev/null || status=1 ;;
  esac
  # Also drain residual children after a normal phase exit.
  stop_validation_units || status=1
  remove_control_rule > "$RESULT/$name-sudoers-cleanup.txt" 2>&1 || status=1
  # Python SIGINT may be a clean systemd exit. Files, not service status, decide.
  local evidence="$RESULT/$name"
  [[ "$name" != adaptive ]] || evidence="$RESULT/adaptive-evaluation"
  [[ "$name" != matrix ]] || evidence="$RESULT/benchmark-matrix"
  python3 "$SOURCE/tools/validation_completion.py" "$name" "$evidence" >> "$RESULT/$name.log" 2>&1 || status=1
  return "$status"
}

main() {
  DIAGNOSTIC_ONLY=0
  if [[ "$#" -gt 0 ]]; then
    [[ "$#" -eq 1 && "$1" == --diagnostic-only ]] || { echo 'Only --diagnostic-only is supported.' >&2; return 1; }
    DIAGNOSTIC_ONLY=1
  fi
  [[ "$EUID" -eq 0 ]] || { echo 'Run once with sudo in the VM terminal.' >&2; return 1; }
  SOURCE=/home/hanss_/cerynth-person4-validation
  RUN_USER=hanss_
  RUN_HOME=$(getent passwd "$RUN_USER" | cut -d: -f6)
  OLD_MATRIX="$SOURCE/artifacts/privileged-validation-20261024T160557Z/benchmark-matrix"
  RULE=/etc/sudoers.d/cerynth-validation
  [[ ! -e "$RULE" && ! -L "$RULE" ]] || { echo 'Existing temporary rule: refusing to overwrite it.' >&2; return 1; }
  for tool in systemctl systemd-run timeout runuser python3 visudo sha256sum flock; do command -v "$tool" >/dev/null; done
  test -x "$SOURCE/target/release/cerynthd"
  test -x "$SOURCE/target/release/cerynth-scx"
  test -x /usr/bin/cerynthctl
  test -x "$SOURCE/tools/validation-ctl"
  test -f "$OLD_MATRIX/plan.json"
  test -f /sys/fs/cgroup/cgroup.controllers
  python3 "$SOURCE/tools/stability_source.py" verify "$SOURCE/stability-source-revision.json"
  if [[ "$DIAGNOSTIC_ONLY" -eq 0 ]]; then
    python3 "$SOURCE/tools/stability_source.py" quarantine "$SOURCE/stability-source-revision.json"
  fi
  exec 9>/run/cerynth/validation.lock
  flock -n 9 || { echo 'Another Cerynth validation driver holds the lock.' >&2; return 1; }
  SESSION="cerynth-validation-$(date -u +%Y%m%dT%H%M%SZ)-$$"
  RESULT="$SOURCE/artifacts/$SESSION"
  BACKUP="$RESULT/runtime-backup"
  INSTALL_TEMP="/usr/lib/cerynth/.cerynthd-install-$SESSION"
  RESTORE_TEMP="/usr/lib/cerynth/.cerynthd-restore-$SESSION"
  INSTALL_SCX_TEMP="/usr/lib/cerynth/.cerynth-scx-install-$SESSION"
  RESTORE_SCX_TEMP="/usr/lib/cerynth/.cerynth-scx-restore-$SESSION"
  umask 022
  mkdir "$RESULT"
  mkdir -m 0700 "$BACKUP"
  MUTATED=0 STATE_EXISTED=0 RULE_CREATED=0 RULE_TEMP='' LAUNCHER_PID='' SAMPLER_PID=''
  UNITS=()
  JOURNAL_CURSOR=$(journalctl -n 1 --no-pager -o json | python3 -c 'import sys,json; print(json.loads(sys.stdin.read())["__CURSOR"])')
  cp "$SOURCE/stability-source-revision.json" "$RESULT/source-revision.json"
  uname -a > "$RESULT/kernel.txt"
  # Snapshot/backup failures occur before any service or privilege mutation.
  runuser -u "$RUN_USER" -- python3 "$SOURCE/tools/validation_manifest.py" "$OLD_MATRIX" > "$RESULT/baseline-before.json"
  cp -p /usr/lib/cerynth/cerynthd "$BACKUP/cerynthd"
  cp -p /usr/lib/cerynth/cerynth-scx "$BACKUP/cerynth-scx"
  cp -p /etc/cerynth/cerynth.toml "$BACKUP/cerynth.toml"
  cat /proc/sys/vm/compact_unevictable_allowed > "$BACKUP/compact_unevictable_allowed"
  if [[ -e /var/lib/cerynth/state.json ]]; then
    cp -p /var/lib/cerynth/state.json "$BACKUP/state.json"
    STATE_EXISTED=1
  fi
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  trap 'exit 129' HUP
  # An already-failed scheduler must not prevent installing the repair.
  if healthy > "$RESULT/initial-health-check.txt" 2>&1; then
    printf 'healthy\n' > "$RESULT/initial-runtime-state.txt"
  else
    printf 'unhealthy-or-linux-fallback; proceeding with guarded repair\n' > "$RESULT/initial-runtime-state.txt"
  fi
  MUTATED=1
  systemctl stop cerynth-policy-live cerynthd
  install -m 0755 "$SOURCE/target/release/cerynthd" "$INSTALL_TEMP"
  mv "$INSTALL_TEMP" /usr/lib/cerynth/cerynthd
  install -m 0755 "$SOURCE/target/release/cerynth-scx" "$INSTALL_SCX_TEMP"
  mv "$INSTALL_SCX_TEMP" /usr/lib/cerynth/cerynth-scx
  systemctl start cerynthd cerynth-policy-live
  start_sampler
  wait_healthy
  ctl adaptation pause
  sha256sum "$BACKUP/cerynthd" > "$RESULT/previous-daemon-sha256.txt"
  sha256sum /usr/lib/cerynth/cerynthd > "$RESULT/installed-daemon-sha256.txt"
  sha256sum "$BACKUP/cerynth-scx" > "$RESULT/previous-scheduler-sha256.txt"
  sha256sum /usr/lib/cerynth/cerynth-scx > "$RESULT/installed-scheduler-sha256.txt"
  chown "$RUN_USER:$(id -gn "$RUN_USER")" "$RESULT"
  if [[ "$DIAGNOSTIC_ONLY" -eq 1 ]]; then
    phase diagnostic "$SOURCE/tools/cerynth-stability.py" --ctl "$SOURCE/tools/validation-ctl" \
      --rounds 3 --duration 15 --idle-soak 120 --linux-baseline 30 --workers 32 --output "$RESULT/diagnostic"
    return 0
  fi
  phase smoke "$SOURCE/tools/cerynth-stability.py" --ctl "$SOURCE/tools/validation-ctl" \
    --rounds 1 --duration 5 --idle-soak 5 --linux-baseline 5 --workers 32 --output "$RESULT/smoke"
  python3 - "$RESULT/smoke/result.json" <<'PYSMOKE'
import hashlib,json,platform,sys
from pathlib import Path
r=json.loads(Path(sys.argv[1]).read_text())
p=r['parameters']
assert r['status']=='pass' and r['completed_phases']==6 and r['watchdog_events']==[]
assert p['workers']>=32 and p['duration']>=5 and p['kernel']==platform.release()
assert p['scheduler_sha256']==hashlib.sha256(Path('/usr/lib/cerynth/cerynth-scx').read_bytes()).hexdigest()
PYSMOKE
  phase stability "$SOURCE/tools/cerynth-stability.py" --ctl "$SOURCE/tools/validation-ctl" \
    --rounds 3 --duration 15 --idle-soak 120 --linux-baseline 30 --workers 32 --output "$RESULT/stability"
  python3 - "$SOURCE" "$RESULT" <<'PYGATE'
import importlib.util, sys
from pathlib import Path
source, result = map(Path,sys.argv[1:])
spec=importlib.util.spec_from_file_location('experiment',source/'benchmarks/experiment.py')
module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.verify_stability_gates([result/'smoke/result.json',result/'stability/result.json'],Path('/usr/lib/cerynth/cerynth-scx'))
PYGATE
  # The bounded observer is needed only for targeted diagnosis, not measurement.
  stop_sampler
  phase adaptive "$SOURCE/benchmarks/adaptive.sh" --output "$RESULT/adaptive-evaluation" \
    --stability-gates "$RESULT/smoke/result.json" "$RESULT/stability/result.json"
  phase matrix "$SOURCE/benchmarks/matrix.sh" --ctl "$SOURCE/tools/validation-ctl" \
    --output "$RESULT/benchmark-matrix" --stability-gates "$RESULT/smoke/result.json" "$RESULT/stability/result.json"
  runuser -u "$RUN_USER" -- "$SOURCE/tools/cerynth-observe" --once --json > "$RESULT/final-observation.json"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then main "$@"; fi
