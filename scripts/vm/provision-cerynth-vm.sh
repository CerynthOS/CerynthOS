#!/usr/bin/env bash
#
# Build the CerynthOS runtime and install it into the running development VM.
#
#   Host                                Guest
#   ----                                -----
#   cargo build --release
#   install-dev-runtime.sh --root TMP
#          |
#          +-- rsync ------------------> /tmp/cerynth-provision
#                                             |
#                                             +-- sudo install --> /
#                                             +-- systemctl daemon-reload
#                                             +-- enable + restart cerynthd
#
# Staging through /tmp keeps the qcow2 image untouched while the VM is live,
# which is much safer than mounting the disk underneath a running kernel.
#
#   ./scripts/vm/provision-cerynth-vm.sh
#   CERYNTH_SKIP_BUILD=1 ./scripts/vm/provision-cerynth-vm.sh

set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# shellcheck source=scripts/vm/cerynth-vm-common.sh
source "$ROOT_DIR/scripts/vm/cerynth-vm-common.sh"

SKIP_BUILD="${CERYNTH_SKIP_BUILD:-0}"
REMOTE_STAGE="/tmp/cerynth-provision"

STAGING_DIR=""

cleanup() {
    [[ -n "$STAGING_DIR" ]] && rm -rf "$STAGING_DIR"
    return 0
}
trap cleanup EXIT

info() {
    printf '[cerynth-provision] %s\n' "$*"
}

for tool in ssh rsync cargo; do
    command -v "$tool" >/dev/null || die "$tool is required on the host"
done

require_vm_key
info "checking VM connection"
require_vm_reachable
require_vm_sudo
info "connected to $(vm_ssh hostname)"

command -v rsync >/dev/null
vm_ssh 'command -v rsync >/dev/null' ||
    die "rsync is not installed in the VM

Fix it inside the VM with:  sudo apt install -y rsync"

if [[ "$SKIP_BUILD" == "1" ]]; then
    info "skipping build (CERYNTH_SKIP_BUILD=1)"
else
    info "building workspace in release mode"
    (cd "$ROOT_DIR" && cargo build --release --workspace)
fi

info "staging runtime on the host"
STAGING_DIR="$(mktemp -d)"
"$ROOT_DIR/scripts/install-dev-runtime.sh" --root "$STAGING_DIR/root" >/dev/null

info "copying runtime to the VM"
vm_ssh "rm -rf '$REMOTE_STAGE' && mkdir -p '$REMOTE_STAGE'"

rsync -a --delete \
    -e "ssh ${SSH_OPTS[*]}" \
    "$STAGING_DIR/root/" \
    "$VM_TARGET:$REMOTE_STAGE/"

info "installing runtime inside the VM"

vm_bash "$REMOTE_STAGE" <<'REMOTE'
set -Eeuo pipefail

STAGE="$1"

sudo install -d -m 0755 /usr/lib/cerynth /etc/cerynth /var/lib/cerynth /run/cerynth

sudo install -m 0755 "$STAGE/usr/lib/cerynth/cerynthd"    /usr/lib/cerynth/cerynthd
sudo install -m 0755 "$STAGE/usr/lib/cerynth/cerynth-scx" /usr/lib/cerynth/cerynth-scx
sudo install -m 0755 "$STAGE/usr/bin/cerynthctl"          /usr/bin/cerynthctl

sudo install -m 0755 \
    "$STAGE/usr/lib/cerynth/cerynth-boot-marker" \
    /usr/lib/cerynth/cerynth-boot-marker

# Existing configuration is user data and is never overwritten.
if [ -f /etc/cerynth/cerynth.toml ]; then
    echo "preserving existing /etc/cerynth/cerynth.toml"
    sudo install -m 0644 "$STAGE/etc/cerynth/cerynth.toml" /etc/cerynth/cerynth.toml.dist
else
    sudo install -m 0644 "$STAGE/etc/cerynth/cerynth.toml" /etc/cerynth/cerynth.toml
fi

sudo install -m 0644 \
    "$STAGE/etc/systemd/system/cerynthd.service" \
    /etc/systemd/system/cerynthd.service

sudo install -m 0644 \
    "$STAGE/etc/systemd/system/cerynth-boot-marker.service" \
    /etc/systemd/system/cerynth-boot-marker.service

sudo systemctl daemon-reload
sudo systemctl enable cerynthd.service cerynth-boot-marker.service

# The marker normally runs at boot. Refreshing it here means the runtime is
# fully consistent immediately after provisioning, without a reboot.
# restart, not start: RemainAfterExit=yes means an already-active oneshot
# would otherwise be skipped and keep a stale marker.
sudo systemctl restart cerynth-boot-marker.service

sudo systemctl restart cerynthd.service

rm -rf "$STAGE"
REMOTE

info "verifying installation"

vm_bash <<'REMOTE'
set -Eeuo pipefail

printf '\nInstalled runtime:\n'
ls -lh \
    /usr/lib/cerynth/cerynthd \
    /usr/lib/cerynth/cerynth-scx \
    /usr/bin/cerynthctl \
    /etc/cerynth/cerynth.toml \
    /etc/systemd/system/cerynthd.service

printf '\nDaemon state: '
systemctl is-active cerynthd.service || true

printf '\nRecent daemon log:\n'
sudo journalctl -u cerynthd -b --no-pager -n 15 || true
REMOTE

printf '\n'
printf 'CerynthOS VM provisioning complete.\n'
printf '\nNext:\n'
printf '  just vm-smoke\n'
