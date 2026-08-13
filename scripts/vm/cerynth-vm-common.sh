#!/usr/bin/env bash
#
# Shared VM connection settings, sourced by the provisioning, smoke test and
# recovery scripts so they can never disagree about how to reach the VM.
#
# Override any of these in the environment:
#
#   CERYNTH_VM_HOST      default 127.0.0.1
#   CERYNTH_VM_USER      default cerynth
#   CERYNTH_VM_SSH_PORT  default 2222
#   CERYNTH_VM_SSH_KEY   default ~/.ssh/cerynth_vm

VM_HOST="${CERYNTH_VM_HOST:-127.0.0.1}"
VM_USER="${CERYNTH_VM_USER:-cerynth}"
VM_PORT="${CERYNTH_VM_SSH_PORT:-2222}"
VM_KEY="${CERYNTH_VM_SSH_KEY:-$HOME/.ssh/cerynth_vm}"

VM_TARGET="$VM_USER@$VM_HOST"

# The VM overlay is disposable and gets recreated by reset-vm.sh, so its host
# key legitimately changes. Known-hosts checking is disabled deliberately.
SSH_OPTS=(
    -i "$VM_KEY"
    -p "$VM_PORT"
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile=/dev/null
    -o LogLevel=ERROR
    -o ConnectTimeout=5
    -o BatchMode=yes
)

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

# Run a command in the VM.
#
# shellcheck disable=SC2029  # remote expansion is intended: callers pass
#                            # commands meant to run in the guest.
vm_ssh() {
    ssh "${SSH_OPTS[@]}" "$VM_TARGET" "$@"
}

# Feed a script to the VM's shell on stdin. Extra arguments become $1, $2, ...
# inside that script. Safer than interpolating paths into a command string.
vm_bash() {
    ssh "${SSH_OPTS[@]}" "$VM_TARGET" bash -s -- "$@"
}

require_vm_key() {
    [[ -f "$VM_KEY" ]] ||
        die "VM SSH key missing: $VM_KEY
Set CERYNTH_VM_SSH_KEY if your key lives elsewhere."
}

require_vm_reachable() {
    vm_ssh true 2>/dev/null ||
        die "cannot reach the VM at $VM_TARGET port $VM_PORT

Is it running?  ./scripts/vm/run-cerynth-vm.sh"
}

# Passwordless sudo is required: every script here runs unattended and would
# otherwise hang forever on a hidden password prompt.
require_vm_sudo() {
    vm_ssh 'sudo -n true' 2>/dev/null ||
        die "passwordless sudo is not available for $VM_USER in the VM

Fix it inside the VM with:
  echo '$VM_USER ALL=(ALL) NOPASSWD:ALL' | sudo tee /etc/sudoers.d/cerynth
  sudo chmod 0440 /etc/sudoers.d/cerynth"
}
