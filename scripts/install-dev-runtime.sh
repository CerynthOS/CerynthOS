#!/usr/bin/env bash
#
# Install the CerynthOS development runtime.
#
# Layout is frozen by docs/contracts/runtime-v1.md:
#
#   /usr/lib/cerynth/cerynthd          daemon
#   /usr/lib/cerynth/cerynth-scx       scheduler
#   /usr/bin/cerynthctl                CLI
#   /etc/cerynth/cerynth.toml          configuration
#   /var/lib/cerynth/                  persistent state
#   /run/cerynth/                      socket and status files
#
# Installing into "/" needs root. Use --root to stage into a directory
# instead, which is how the VM provisioner builds its payload.

set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

INSTALL_ROOT="/"
VERBOSE=0
ENABLE_UNITS=1

usage() {
    cat <<USAGE
Usage:
  $0 [--root PATH] [--no-enable] [--verbose]

Options:
  --root PATH   Install into PATH instead of / (no root required)
  --no-enable   Do not run systemctl enable/daemon-reload
  --verbose     Print each file as it is installed

Examples:
  sudo $0
  $0 --root /tmp/cerynth-root
USAGE
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

info() {
    printf '[cerynth-install] %s\n' "$*"
}

verbose() {
    [[ "$VERBOSE" -eq 1 ]] && printf '[cerynth-install]   %s\n' "$*"
    return 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --root)
            [[ $# -ge 2 ]] || die "--root requires a path"
            INSTALL_ROOT="$2"
            shift 2
            ;;
        --no-enable)
            ENABLE_UNITS=0
            shift
            ;;
        --verbose)
            VERBOSE=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
done

if [[ "$INSTALL_ROOT" != "/" ]]; then
    INSTALL_ROOT="$(realpath -m "$INSTALL_ROOT")"
    # Strip a trailing slash so "$INSTALL_ROOT/usr" never becomes "//usr".
    INSTALL_ROOT="${INSTALL_ROOT%/}"
    [[ -n "$INSTALL_ROOT" ]] && ENABLE_UNITS=0
fi

# Path prefix that is empty for "/" and the staging root otherwise.
PREFIX="$INSTALL_ROOT"
[[ "$PREFIX" == "/" ]] && PREFIX=""

DEST_LIB="$PREFIX/usr/lib/cerynth"
DEST_BIN="$PREFIX/usr/bin"
DEST_ETC="$PREFIX/etc/cerynth"
DEST_STATE="$PREFIX/var/lib/cerynth"
DEST_RUN="$PREFIX/run/cerynth"
DEST_SYSTEMD="$PREFIX/etc/systemd/system"

DAEMON="$ROOT_DIR/target/release/cerynthd"
CLI="$ROOT_DIR/target/release/cerynthctl"
SCHEDULER="$ROOT_DIR/target/release/cerynth-scx"

CONFIG="$ROOT_DIR/packaging/config/cerynth.toml"
SERVICE="$ROOT_DIR/packaging/systemd/cerynthd.service"
BOOT_MARKER="$ROOT_DIR/packaging/systemd/cerynth-boot-marker.service"
BOOT_MARKER_SCRIPT="$ROOT_DIR/packaging/scripts/cerynth-boot-marker"

for binary in "$DAEMON" "$CLI" "$SCHEDULER"; do
    [[ -f "$binary" ]] || die "missing $binary
Build it first:  cargo build --release --workspace"
done

for file in "$CONFIG" "$SERVICE" "$BOOT_MARKER" "$BOOT_MARKER_SCRIPT"; do
    [[ -f "$file" ]] || die "required file missing: $file"
done

if [[ -z "$PREFIX" && "$(id -u)" -ne 0 ]]; then
    die "installing into / requires root; use sudo or --root"
fi

info "installation root: ${INSTALL_ROOT}"

install -d -m 0755 \
    "$DEST_LIB" \
    "$DEST_BIN" \
    "$DEST_ETC" \
    "$DEST_STATE" \
    "$DEST_RUN" \
    "$DEST_SYSTEMD"

info "installing binaries"
install -m 0755 "$DAEMON" "$DEST_LIB/cerynthd"
verbose "$DEST_LIB/cerynthd"
install -m 0755 "$SCHEDULER" "$DEST_LIB/cerynth-scx"
verbose "$DEST_LIB/cerynth-scx"
install -m 0755 "$CLI" "$DEST_BIN/cerynthctl"
verbose "$DEST_BIN/cerynthctl"
install -m 0755 "$BOOT_MARKER_SCRIPT" "$DEST_LIB/cerynth-boot-marker"
verbose "$DEST_LIB/cerynth-boot-marker"

# An existing config is user data: never overwrite it. Ship the new default
# alongside as .dist so upgrades stay visible without being destructive.
if [[ -f "$DEST_ETC/cerynth.toml" ]]; then
    info "preserving existing configuration"
    install -m 0644 "$CONFIG" "$DEST_ETC/cerynth.toml.dist"
    verbose "$DEST_ETC/cerynth.toml.dist"
else
    info "installing default configuration"
    install -m 0644 "$CONFIG" "$DEST_ETC/cerynth.toml"
    verbose "$DEST_ETC/cerynth.toml"
fi

info "installing systemd units"
install -m 0644 "$SERVICE" "$DEST_SYSTEMD/cerynthd.service"
verbose "$DEST_SYSTEMD/cerynthd.service"
install -m 0644 "$BOOT_MARKER" "$DEST_SYSTEMD/cerynth-boot-marker.service"
verbose "$DEST_SYSTEMD/cerynth-boot-marker.service"

if [[ "$ENABLE_UNITS" -eq 1 ]]; then
    info "reloading systemd"
    systemctl daemon-reload

    info "enabling units"
    systemctl enable cerynthd.service cerynth-boot-marker.service
fi

printf '\n'
printf 'CerynthOS development runtime installed:\n'
printf '  %-20s %s\n' 'Daemon:'      "$DEST_LIB/cerynthd"
printf '  %-20s %s\n' 'Scheduler:'   "$DEST_LIB/cerynth-scx"
printf '  %-20s %s\n' 'CLI:'         "$DEST_BIN/cerynthctl"
printf '  %-20s %s\n' 'Config:'      "$DEST_ETC/cerynth.toml"
printf '  %-20s %s\n' 'State dir:'   "$DEST_STATE"
printf '  %-20s %s\n' 'Runtime dir:' "$DEST_RUN"
printf '  %-20s %s\n' 'Units:'       "$DEST_SYSTEMD/cerynthd.service"
printf '  %-20s %s\n' ''             "$DEST_SYSTEMD/cerynth-boot-marker.service"

if [[ "$ENABLE_UNITS" -eq 1 ]]; then
    printf '\nNext:\n'
    printf '  systemctl start cerynthd\n'
    printf '  cerynthctl status\n'
fi
