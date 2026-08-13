# CerynthOS Runtime: Operation and Recovery

How to provision the development VM, and how to get it back when the runtime
or the scheduler misbehaves.

## Normal workflow

```bash
# 1. Boot the VM (foreground; keep this terminal open)
./scripts/vm/run-cerynth-vm.sh

# 2. In another terminal: build + install the runtime into the VM
just vm-provision

# 3. Verify everything
just vm-smoke
```

Expected result:

```text
Passed: 8
Failed: 0

SMOKE TEST PASSED
```

Artifacts land in `artifacts/smoke-test/`:

| File | Contents |
| --- | --- |
| `result.json` | Machine-readable per-check results |
| `system-info.txt` | Kernel, cmdline, units, config, sched_ext state |
| `journal.log` | Full `cerynthd` journal for the current boot |
| `dmesg.log` | Kernel ring buffer |

## VM connection settings

All scripts share `scripts/vm/cerynth-vm-common.sh`. Override via
environment:

| Variable | Default |
| --- | --- |
| `CERYNTH_VM_HOST` | `127.0.0.1` |
| `CERYNTH_VM_USER` | `cerynth` |
| `CERYNTH_VM_SSH_PORT` | `2222` |
| `CERYNTH_VM_SSH_KEY` | `~/.ssh/cerynth_vm` |
| `CERYNTH_SKIP_BUILD` | unset; set to `1` to skip `cargo build` |

## Prerequisites inside the VM

The provisioner runs unattended and checks these up front rather than
hanging on a hidden prompt:

```bash
# Passwordless sudo
echo 'cerynth ALL=(ALL) NOPASSWD:ALL' | sudo tee /etc/sudoers.d/cerynth
sudo chmod 0440 /etc/sudoers.d/cerynth

# rsync
sudo apt install -y rsync
```

## Recovery

### One-shot recovery

```bash
just vm-recover
```

This stops `cerynthd`, terminates `cerynth-scx` with SIGTERM (escalating to
SIGKILL after 10s), confirms sched_ext handed control back to Linux, removes
stale sockets and status files, clears failed unit state, and restarts the
daemon.

To stop everything and stay down:

```bash
./scripts/vm/runtime-recovery.sh --no-restart
```

### `cerynthd` will not start

```bash
sudo journalctl -u cerynthd -b --no-pager -n 100
sudo systemd-analyze verify /etc/systemd/system/cerynthd.service
ls -la /run/cerynth /var/lib/cerynth /etc/cerynth
```

Common causes:

- **Stale socket.** A previous unclean exit left `cerynthd.sock` behind. The
  daemon unlinks it at startup, but a leftover owned by another user will
  block the bind. `sudo rm -f /run/cerynth/cerynthd.sock`.
- **Unparseable config.** The daemon logs
  `warning: cannot parse config ...` and falls back to defaults. It keeps
  running, so check the journal rather than assuming the config took effect.
- **Missing state directory.** `StateDirectory=cerynth` should create
  `/var/lib/cerynth`; if it is missing, `systemctl daemon-reload` and restart.

### `cerynthctl` reports "Could not connect"

```bash
ls -l /run/cerynth/cerynthd.sock     # expect srw-rw---- root root
systemctl is-active cerynthd
```

The socket is mode `0660` owned by root, and connecting to a Unix socket
needs write permission, so the CLI requires root:

```bash
sudo cerynthctl status
```

If the socket is missing entirely but the daemon is active, the daemon
failed to bind — check the journal.

### Scheduler is stuck / system feels wedged

```bash
cat /sys/kernel/sched_ext/state
```

- `disabled` — Linux is scheduling. Nothing attached.
- `enabled` — a BPF scheduler is attached.

To hand control back to Linux:

```bash
sudo pkill -TERM -x cerynth-scx
cat /sys/kernel/sched_ext/state    # expect: disabled
```

The kernel detaches a sched_ext scheduler when its loading process exits, so
killing `cerynth-scx` is always sufficient. sched_ext also has a watchdog
that ejects a scheduler that stops making progress, which is why the machine
stays usable even when a scheduler misbehaves.

If `enabled` persists after the process is gone, reboot the VM. Do not treat
that as normal — capture `dmesg` first, it indicates a scheduler bug worth
reporting to Person 2.

### Total reset

This destroys everything inside the VM.

```bash
# Shut the VM down first; reset-vm.sh refuses while QEMU is running
ssh -i ~/.ssh/cerynth_vm -p 2222 cerynth@127.0.0.1 'sudo poweroff'

./scripts/vm/reset-vm.sh     # type RESET to confirm

./scripts/vm/run-cerynth-vm.sh
just vm-provision
just vm-smoke
```

The overlay is recreated from `vm/images/ubuntu-noble-base.img`, so the VM
returns to pristine Ubuntu and the passwordless-sudo and rsync prerequisites
above must be applied again.

## Distro kernel fallback

The custom kernel is supplied by QEMU with `-kernel`, not installed into the
guest, so the VM's own GRUB entries and distro kernel are untouched. Booting
the stock kernel means running the base VM instead:

```bash
./scripts/vm/run-base-vm.sh
```

CerynthOS cannot render the guest unbootable through the kernel path,
because the guest never boots the custom kernel on its own.

## A note on rebooting

`run-cerynth-vm.sh` passes `-no-reboot` to QEMU, so `sudo reboot` inside the
VM **terminates the VM** rather than restarting it. To test boot
persistence, let it exit and start it again:

```bash
ssh -i ~/.ssh/cerynth_vm -p 2222 cerynth@127.0.0.1 'sudo reboot'
# wait for QEMU to exit, then:
./scripts/vm/run-cerynth-vm.sh
just vm-smoke        # cerynthd must pass without being started by hand
```
