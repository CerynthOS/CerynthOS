#!/usr/bin/env bash
#
# Verify a CerynthOS runtime layout.
#
# Works against a staging root produced by install-dev-runtime.sh --root,
# or against "/" on a provisioned machine.
#
#   ./scripts/check-runtime-files.sh /tmp/cerynth-install-test
#   ./scripts/check-runtime-files.sh          # checks /

set -Eeuo pipefail

ROOT="${1:-/}"
ROOT="${ROOT%/}"

FAILED=0

ok() {
    printf '[OK]   %s\n' "$1"
}

miss() {
    printf '[MISS] %s (%s)\n' "$1" "$2"
    FAILED=1
}

check_exec() {
    if [[ -x "$ROOT$1" ]]; then
        ok "$1"
    elif [[ -e "$ROOT$1" ]]; then
        miss "$1" "exists but is not executable"
    else
        miss "$1" "not found"
    fi
}

check_file() {
    if [[ -f "$ROOT$1" ]]; then
        ok "$1"
    else
        miss "$1" "not found"
    fi
}

check_dir() {
    if [[ -d "$ROOT$1" ]]; then
        ok "$1"
    else
        miss "$1" "not found"
    fi
}

printf 'Checking CerynthOS runtime under: %s\n\n' "${ROOT:-/}"

check_exec /usr/lib/cerynth/cerynthd
check_exec /usr/lib/cerynth/cerynth-scx
check_exec /usr/bin/cerynthctl
check_exec /usr/lib/cerynth/cerynth-boot-marker

check_file /etc/cerynth/cerynth.toml

check_file /etc/systemd/system/cerynthd.service
check_file /etc/systemd/system/cerynth-boot-marker.service

check_dir /var/lib/cerynth
check_dir /run/cerynth

printf '\n'

if [[ "$FAILED" -eq 0 ]]; then
    printf 'Runtime layout OK.\n'
else
    printf 'Runtime layout INCOMPLETE.\n' >&2
fi

exit "$FAILED"
