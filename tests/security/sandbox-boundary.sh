#!/usr/bin/env bash
#
# Cerynth Guard boundary tests for scripts/guard/guard-sandbox.sh.
#
# Every test runs a command inside the sandbox and checks the outcome.
# "allow" tests must succeed: a sandbox that blocks legitimate work is a
# failure too. "deny" tests are attacks and must fail.
#
# Planted canary secrets live in a throwaway fake home, so the tests never
# read real credentials. One extra check confirms the real ~/.ssh is not
# visible inside the sandbox (without reading it outside).
#
#   tests/security/sandbox-boundary.sh            # all tests
#   KEEP=1 tests/security/sandbox-boundary.sh     # keep the temp dirs
#
# Exit code: 0 if every test passed, 1 otherwise. Unprivileged; no sudo.

set -Euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# GUARD_LAUNCHER=target/debug/cerynth-guard-run runs the same tests through
# the Rust launcher, which adds Landlock (L4) and seccomp (L5) and enables
# the extra layer tests at the end.
SANDBOX="${GUARD_LAUNCHER:-$ROOT/scripts/guard/guard-sandbox.sh}"

TMP="$(mktemp -d "${TMPDIR:-/tmp}/cerynth-guard-test.XXXXXX")"
WS="$TMP/workspace"
FAKE_HOME="$TMP/fake-home"
cleanup() { [[ "${KEEP:-0}" == 1 ]] && echo "kept: $TMP" || rm -rf "$TMP"; }
trap cleanup EXIT

pass=0; fail=0
result() { # name expected(allow|deny) rc
    local ok=0
    if [[ "$2" == allow && "$3" -eq 0 ]] || [[ "$2" == deny && "$3" -ne 0 ]]; then ok=1; fi
    if ((ok)); then pass=$((pass + 1)); printf 'PASS  %-5s %s\n' "$2" "$1"
    else fail=$((fail + 1)); printf 'FAIL  %-5s %s (exit %s)\n' "$2" "$1" "$3"; fi
}
sbx() { "$SANDBOX" --workspace "$WS" -- "$@" >/dev/null 2>&1; }
check() { # name allow|deny shell-snippet-run-inside-sandbox
    sbx bash -c "$3"; result "$1" "$2" $?
}

# --- fixtures ---------------------------------------------------------------
mkdir -p "$WS" "$FAKE_HOME/.ssh" "$FAKE_HOME/.config/gh"
echo "CANARY-SSH-KEY-7f3a" > "$FAKE_HOME/.ssh/id_ed25519"
echo "oauth_token: CANARY-GH-TOKEN-91c2" > "$FAKE_HOME/.config/gh/hosts.yml"
cat > "$WS/calc.py" <<'EOF'
def add(a, b):
    return a - b   # bug: the agent's task is to fix this
EOF
cat > "$WS/test_calc.py" <<'EOF'
import unittest
from calc import add
class T(unittest.TestCase):
    def test_add(self):
        self.assertEqual(add(2, 3), 5)
if __name__ == "__main__":
    unittest.main()
EOF

echo "Cerynth Guard sandbox boundary tests"
echo "workspace: $WS"
echo

# --- legitimate work: must succeed -------------------------------------------
check "read the workspace"                 allow 'cat /workspace/calc.py'
check "fix the bug in the workspace"       allow "sed -i 's/a - b/a + b/' /workspace/calc.py"
check "run the project's tests"            allow 'cd /workspace && python3 -m unittest -q test_calc'
check "create files in the workspace"      allow 'echo patch > /workspace/result.patch'
check "use private /tmp and \$HOME"        allow 'echo x > /tmp/a && echo y > "$HOME/b" && cat /tmp/a "$HOME/b"'
check "run git in the workspace"           allow 'cd /workspace && git init -q && git add -A && git -c user.email=a@b -c user.name=agent commit -qm fix'
check "spawn child processes"              allow 'bash -c "python3 -c \"print(1)\""'
# The fix really landed on the host side of the bind mount:
grep -q 'a + b' "$WS/calc.py"; result "workspace edit visible to the host" allow $?

# Exits 0 if io_uring_setup is reachable (the kernel answers EFAULT for the
# NULL params), 1 if something blocked it with EPERM.
IOURING_PROBE='python3 -c "import ctypes,sys; l=ctypes.CDLL(None,use_errno=True); r=l.syscall(425,8,ctypes.c_void_p(0)); sys.exit(1 if r<0 and ctypes.get_errno()==1 else 0)"'

# --- controls: the same attacks must WORK outside the sandbox -----------------
# Otherwise a "deny" result below could just mean the command was broken.
ctl() { bash -c "$2" >/dev/null 2>&1; local rc=$?; ((rc == 0)) && printf 'ok    ctrl  %s\n' "$1" || { fail=$((fail + 1)); printf 'FAIL  ctrl  %s (control did not work outside the sandbox, test is meaningless)\n' "$1"; }; }
ctl "planted SSH key is readable on the host"   "cat '$FAKE_HOME/.ssh/id_ed25519'"
ctl "planted token is readable on the host"     "cat '$FAKE_HOME/.config/gh/hosts.yml'"
ctl "real home directory is listable on the host" "ls '$HOME'"
ctl "../ path resolves on the host"             "cat '$WS/../fake-home/.ssh/id_ed25519'"
FAKE_API_KEY="CANARY-API-KEY-55d1" bash -c 'env | grep -q CANARY'; ((${PIPESTATUS[0]:-$?} == 0)) && echo "ok    ctrl  API key visible in a normal child environment" || { fail=$((fail + 1)); echo "FAIL  ctrl  API key control"; }
if [[ "$(basename "$SANDBOX")" == cerynth-guard-run ]]; then
    ctl "unshare -U works on the host"            'unshare -U true'
    ctl "io_uring_setup is reachable on the host" "$IOURING_PROBE"
fi
if timeout 5 bash -c 'exec 3<>/dev/tcp/1.1.1.1/443' 2>/dev/null; then
    echo "ok    ctrl  network reachable on the host"
else
    echo "skip  ctrl  host is offline, so the network deny test only proves little"
fi
echo

# --- attacks: must fail -------------------------------------------------------
check "read a planted SSH key by absolute path" deny "cat '$FAKE_HOME/.ssh/id_ed25519'"
check "read a planted GitHub token"             deny "cat '$FAKE_HOME/.config/gh/hosts.yml'"
check "list the real home directory"            deny "ls '$HOME'"
check "see the real ~/.ssh"                     deny "test -e '$HOME/.ssh'"
check "child process reads the planted key"     deny "bash -c \"cat '$FAKE_HOME/.ssh/id_ed25519'\""
check "escape via ../ from the workspace"       deny "cat /workspace/../fake-home/.ssh/id_ed25519"
check "write to system directories"             deny 'echo x > /usr/cerynth-pwned'
check "modify /etc"                             deny 'echo x >> /etc/hosts'
check "reach the network (TCP to 1.1.1.1:443)"  deny 'timeout 5 bash -c "exec 3<>/dev/tcp/1.1.1.1/443"'
check "resolve DNS"                             deny 'timeout 5 getent hosts github.com'
check "reach Cerynth control sockets (/run/cerynth)" deny 'test -e /run/cerynth'
check "read Cerynth persistent state"           deny 'test -e /var/lib/cerynth'
check "see host processes (PID 1 is not init)"  deny '[ "$(cat /proc/1/comm)" = systemd ]'
check "gain root privileges via sudo"           deny 'sudo -n true'
check "regain capabilities (CapEff non-zero)"   deny 'grep -q "^CapEff:\s*0*[1-9a-f]" /proc/self/status'
if [[ -e /dev/dri/renderD128 ]]; then
    check "access host GPU render node without a grant" deny 'test -e /dev/dri/renderD128'
else
    echo "skip  GPU  no host render node; GPU access denial not validated"
fi

# Credentials in the launcher's environment must not reach the agent.
FAKE_API_KEY="CANARY-API-KEY-55d1" OPENAI_API_KEY="CANARY-OPENAI-0b7e" \
    "$SANDBOX" --workspace "$WS" -- bash -c 'env | grep -q CANARY' >/dev/null 2>&1
result "inherit API keys from the launcher's environment" deny $?

# --- layers L4/L5: only checked with the Rust launcher -------------------------
if [[ "$(basename "$SANDBOX")" == cerynth-guard-run ]]; then
    echo
    echo "Layer tests (Landlock + seccomp):"
    # /run and /home are writable tmpfs mounts, so only Landlock stops these.
    check "Landlock: write outside the granted paths (/run)"  deny 'touch /run/escape'
    check "Landlock: write next to \$HOME (/home)"            deny 'touch /home/escape'
    check "Landlock: still allows the workspace"              allow 'touch /workspace/landlock-ok'
    check "no_new_privs is set"                               allow 'grep -q "^NoNewPrivs:\s*1" /proc/self/status'
    check "seccomp filter is active"                          allow 'grep -q "^Seccomp:\s*2" /proc/self/status'
    check "seccomp: create a user namespace (unshare -U)"     deny 'unshare -U true'
    check "seccomp: create a mount namespace via unshare -m"  deny 'unshare -m true'
    check "seccomp: create a namespace via clone (python os.unshare)" deny 'python3 -c "import os; os.unshare(os.CLONE_NEWUSER)"'
    check "seccomp: io_uring is unavailable"                  deny "$IOURING_PROBE"
fi

# The canaries must not have leaked into the workspace either.
! grep -rq 'CANARY-' "$WS" 2>/dev/null; result "no canary data in the workspace afterwards" allow $?

echo
echo "$pass passed, $fail failed"
((fail == 0))
