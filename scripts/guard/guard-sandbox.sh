#!/usr/bin/env bash
#
# Cerynth Guard sandbox prototype (Sprint 1, P2-02).
#
# Runs a command as an untrusted agent would run: inside its own user, PID,
# IPC, UTS and network namespaces, seeing only
#
#   - the host's system directories, read-only (/usr, /etc, /bin, /lib ...)
#   - ONE workspace directory, read-write, mounted at /workspace
#   - a private, empty /tmp and $HOME
#
# Nothing else from the host filesystem is visible: no other home
# directories, no ~/.ssh, no /run (so no Cerynth control sockets), no
# /var/lib/cerynth. The environment is cleared and only an allowlist is
# passed in, so API keys in the caller's environment do not leak inside.
#
#   scripts/guard/guard-sandbox.sh --workspace DIR [--net] -- CMD [ARGS...]
#
# --net keeps host networking (the default is no network: loopback only).
#
# This is a prototype of the boundary, not the final launcher: cerynth-guardd
# will build the same profile from a SecurityDecision, add Landlock and a
# seccomp filter, and place the process in Person 1's resource scope.

set -Eeuo pipefail

die() { printf 'guard-sandbox: %s\n' "$*" >&2; exit 2; }

WORKSPACE=""
NET=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --workspace) WORKSPACE="${2:-}"; shift 2 ;;
        --net) NET=1; shift ;;
        --) shift; break ;;
        *) die "unknown option: $1" ;;
    esac
done
[[ $# -gt 0 ]] || die "no command given (use -- CMD ...)"
[[ -n "$WORKSPACE" && -d "$WORKSPACE" ]] || die "--workspace must be an existing directory"
command -v bwrap >/dev/null || die "bubblewrap (bwrap) is required"

WORKSPACE="$(cd "$WORKSPACE" && pwd -P)"

args=(
    --unshare-user --unshare-pid --unshare-ipc --unshare-uts --unshare-cgroup-try
    --hostname cerynth-agent
    --die-with-parent
    --new-session                 # no controlling tty: blocks TIOCSTI injection
    --cap-drop ALL
    --clearenv
    --setenv PATH /usr/local/bin:/usr/bin:/bin
    --setenv HOME /home/agent
    --setenv USER agent
    --setenv LANG "${LANG:-C.UTF-8}"
    --setenv TERM "${TERM:-dumb}"
    --setenv CERYNTH_SANDBOX 1
    --proc /proc
    --dev /dev
    --tmpfs /tmp
    --tmpfs /home
    --dir /home/agent
    --bind "$WORKSPACE" /workspace
    --chdir /workspace
)
[[ "$NET" == 1 ]] || args+=(--unshare-net)

# System directories, read-only. Only those that exist on this host.
for d in /usr /bin /sbin /lib /lib64 /lib32 /etc /opt; do
    [[ -e "$d" ]] && args+=(--ro-bind "$d" "$d")
done
# DNS for --net mode on systemd-resolved hosts.
if [[ "$NET" == 1 && -e /run/systemd/resolve ]]; then
    args+=(--ro-bind /run/systemd/resolve /run/systemd/resolve)
fi

exec bwrap "${args[@]}" -- "$@"
