# CerynthOS VM Boot Guide

This guide explains how to prepare, boot, validate, recover, and reproduce the CerynthOS development VM on another Linux machine.

The target result is:

```text
Custom CerynthOS Linux kernel boots in QEMU/KVM
        ↓
Ubuntu userspace starts successfully
        ↓
SSH access works
        ↓
BTF is available
        ↓
sched_ext is available
        ↓
CERYNTH_BOOT_OK marker is created
```

The tested success state is:

```text
Kernel: 7.1.0
systemd: running
BTF: available
sched_ext: disabled
boot marker: present
```

`sched_ext` showing `disabled` is expected before a custom scheduler is loaded.

---

## 1. Repository layout

The commands in this guide assume the repository resembles:

```text
CerynthOS/
├── artifacts/
├── kernel/
│   └── build/
│       ├── .config
│       ├── System.map
│       ├── vmlinux
│       └── arch/x86/boot/bzImage
├── packaging/
│   └── systemd/
├── scripts/
│   └── vm/
├── third_party/
│   └── linux/
└── vm/
    ├── cloud-init/
    ├── images/
    ├── initramfs/
    ├── logs/
    ├── modules/
    └── overlays/
```

Run all host-side commands from the repository root.

---

## 2. Requirements

Recommended host:

- x86_64 Linux
- Ubuntu or Debian-based distribution
- Hardware virtualization enabled in BIOS/UEFI
- At least 4 CPU threads
- At least 8 GB RAM
- At least 35 GB free disk space
- Internet connection for the initial Ubuntu image download

Install the required packages:

```bash
sudo apt update

sudo apt install -y \
  qemu-system-x86 \
  qemu-utils \
  cloud-image-utils \
  cpu-checker \
  openssh-client \
  rsync \
  curl \
  wget \
  jq \
  socat \
  gawk \
  build-essential \
  bc \
  bison \
  flex \
  libssl-dev \
  libelf-dev \
  libncurses-dev \
  dwarves \
  pkg-config
```

Verify the tools:

```bash
qemu-system-x86_64 --version
qemu-img --version
cloud-localds --version
```

---

## 3. Verify KVM acceleration

Run:

```bash
kvm-ok
```

Check the KVM device:

```bash
test -e /dev/kvm \
  && echo "KVM device exists" \
  || echo "KVM unavailable"
```

Check permissions:

```bash
ls -l /dev/kvm
groups
```

If the current user is not in the `kvm` group:

```bash
sudo usermod -aG kvm "$USER"
```

Log out and back in, then verify:

```bash
test -r /dev/kvm && test -w /dev/kvm \
  && echo "KVM ready" \
  || echo "KVM permission problem"
```

The scripts can use software emulation when KVM is unavailable, but it will be much slower.

---

## 4. Verify the custom kernel

Set reusable paths:

```bash
LINUX_SRC="$PWD/third_party/linux"
KERNEL_BUILD="$PWD/kernel/build"
```

Verify the source tree:

```bash
test -f "$LINUX_SRC/Makefile" \
  && echo "Linux source tree found" \
  || echo "Linux source tree missing"
```

Verify the build artifacts:

```bash
ls -lh \
  "$KERNEL_BUILD/vmlinux" \
  "$KERNEL_BUILD/arch/x86/boot/bzImage"
```

Expected example:

```text
kernel/build/arch/x86/boot/bzImage   22M
kernel/build/vmlinux                 506M
```

Record the kernel release:

```bash
KERNEL_RELEASE="$(
  make -s -C "$LINUX_SRC" \
    O="$KERNEL_BUILD" \
    kernelrelease
)"

echo "$KERNEL_RELEASE"

mkdir -p artifacts
printf '%s\n' "$KERNEL_RELEASE" | tee artifacts/kernel-release.txt
```

Verify the important kernel options:

```bash
grep -E \
'^CONFIG_(SCHED_CLASS_EXT|DEBUG_INFO_BTF|BPF|BPF_SYSCALL|BPF_JIT)=' \
"$KERNEL_BUILD/.config"
```

Required or strongly recommended:

```text
CONFIG_BPF=y
CONFIG_BPF_SYSCALL=y
CONFIG_BPF_JIT=y
CONFIG_SCHED_CLASS_EXT=y
CONFIG_DEBUG_INFO_BTF=y
```

Check VM-critical drivers:

```bash
grep -E \
'^CONFIG_(VIRTIO|VIRTIO_PCI|VIRTIO_BLK|VIRTIO_NET|EXT4_FS|SERIAL_8250|SERIAL_8250_CONSOLE)=' \
"$KERNEL_BUILD/.config"
```

For the simplest VM boot, these should preferably be built directly into the kernel with `=y`.

---

## 5. Kernel build notes

The actual Linux source tree is:

```text
third_party/linux
```

The out-of-tree kernel build directory is:

```text
kernel/build
```

To build or resume the kernel:

```bash
make -C "$PWD/third_party/linux" \
  O="$PWD/kernel/build" \
  -j"$(nproc)" \
  2>&1 | tee kernel-build.log
```

If the build fails because of Ubuntu-specific certificate paths:

```text
No rule to make target 'debian/canonical-certs.pem'
```

clear the certificate configuration:

```bash
third_party/linux/scripts/config \
  --file kernel/build/.config \
  --set-str SYSTEM_TRUSTED_KEYS "" \
  --set-str SYSTEM_REVOCATION_KEYS ""

make -C "$PWD/third_party/linux" \
  O="$PWD/kernel/build" \
  olddefconfig
```

Verify:

```bash
grep -E '^CONFIG_SYSTEM_(TRUSTED_KEYS|REVOCATION_KEYS)=' \
  kernel/build/.config
```

Expected:

```text
CONFIG_SYSTEM_TRUSTED_KEYS=""
CONFIG_SYSTEM_REVOCATION_KEYS=""
```

If the build reports:

```text
/bin/sh: 1: gawk: not found
```

install it:

```bash
sudo apt install -y gawk
```

Then rerun the build without deleting `kernel/build`.

---

## 6. Create VM directories

```bash
mkdir -p \
  vm/images \
  vm/overlays \
  vm/cloud-init \
  vm/initramfs \
  vm/logs \
  vm/modules \
  scripts/vm \
  artifacts/boot-report
```

Add generated VM files to `.gitignore`:

```bash
cat >> .gitignore <<'EOF'

# CerynthOS VM artifacts
vm/images/*.img
vm/images/*.qcow2
vm/overlays/*.qcow2
vm/cloud-init/*.iso
vm/initramfs/initrd.img-*
vm/logs/*
vm/modules/*
artifacts/boot-report/*

!vm/images/.gitkeep
!vm/overlays/.gitkeep
!vm/initramfs/.gitkeep
!vm/logs/.gitkeep
!vm/modules/.gitkeep
!artifacts/boot-report/.gitkeep
EOF
```

Create placeholders:

```bash
touch \
  vm/images/.gitkeep \
  vm/overlays/.gitkeep \
  vm/initramfs/.gitkeep \
  vm/logs/.gitkeep \
  vm/modules/.gitkeep \
  artifacts/boot-report/.gitkeep
```

Do not commit VM disk images, private SSH keys, or large logs.

---

## 7. Download the Ubuntu base image

This guide uses Ubuntu 24.04 LTS Noble.

```bash
wget \
  --continue \
  --output-document=vm/images/ubuntu-noble-base.img \
  https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img
```

Inspect the image:

```bash
qemu-img info vm/images/ubuntu-noble-base.img
```

Resize it:

```bash
qemu-img resize vm/images/ubuntu-noble-base.img 30G
```

Make the base image read-only:

```bash
chmod a-w vm/images/ubuntu-noble-base.img
```

---

## 8. Create a writable overlay

Never modify the base image directly.

```bash
qemu-img create \
  -f qcow2 \
  -F qcow2 \
  -b "$PWD/vm/images/ubuntu-noble-base.img" \
  "$PWD/vm/overlays/cerynth-dev.qcow2"
```

Verify:

```bash
qemu-img info vm/overlays/cerynth-dev.qcow2
```

---

## 9. Create a VM-specific SSH key

```bash
ssh-keygen \
  -t ed25519 \
  -f "$HOME/.ssh/cerynth_vm" \
  -C "cerynth-vm" \
  -N ""
```

Verify:

```bash
ls -l \
  "$HOME/.ssh/cerynth_vm" \
  "$HOME/.ssh/cerynth_vm.pub"
```

Never commit the private key.

---

## 10. Create cloud-init configuration

Create metadata:

```bash
cat > vm/cloud-init/meta-data <<'EOF'
instance-id: cerynth-dev-01
local-hostname: cerynth-vm
EOF
```

Load the public key:

```bash
CERYNTH_SSH_KEY="$(cat "$HOME/.ssh/cerynth_vm.pub")"
```

Create user data:

```bash
cat > vm/cloud-init/user-data <<EOF
#cloud-config

hostname: cerynth-vm
manage_etc_hosts: true

users:
  - name: cerynth
    gecos: CerynthOS Developer
    groups:
      - sudo
      - adm
      - systemd-journal
    shell: /bin/bash
    sudo:
      - ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys:
      - ${CERYNTH_SSH_KEY}

ssh_pwauth: false
disable_root: true

package_update: true

packages:
  - openssh-server
  - initramfs-tools
  - kmod
  - rsync
  - jq
  - curl
  - git
  - build-essential
  - linux-tools-common

growpart:
  mode: auto
  devices:
    - /

resize_rootfs: true

write_files:
  - path: /etc/motd
    permissions: '0644'
    content: |
      CerynthOS Development VM
      Custom kernel test environment

runcmd:
  - systemctl enable ssh
  - systemctl start ssh
  - mkdir -p /opt/cerynth
  - mkdir -p /run/cerynth
  - touch /var/log/cerynth-cloud-init-complete
  - echo "CERYNTH_BASE_VM_READY" > /dev/console

final_message: "CerynthOS base VM initialization complete"
EOF
```

Generate the seed image:

```bash
cloud-localds \
  vm/cloud-init/seed.iso \
  vm/cloud-init/user-data \
  vm/cloud-init/meta-data
```

Verify:

```bash
ls -lh vm/cloud-init/seed.iso
```

---

## 11. Base VM launcher

Create `scripts/vm/run-base-vm.sh`:

```bash
cat > scripts/vm/run-base-vm.sh <<'EOF'
#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

DISK="${CERYNTH_VM_DISK:-$ROOT_DIR/vm/overlays/cerynth-dev.qcow2}"
SEED="${CERYNTH_CLOUD_INIT:-$ROOT_DIR/vm/cloud-init/seed.iso}"
MEMORY="${CERYNTH_VM_MEMORY:-6G}"
CPUS="${CERYNTH_VM_CPUS:-4}"
SSH_PORT="${CERYNTH_VM_SSH_PORT:-2222}"

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

[[ -f "$DISK" ]] || die "VM disk missing: $DISK"
[[ -f "$SEED" ]] || die "cloud-init seed missing: $SEED"

if [[ -r /dev/kvm && -w /dev/kvm ]]; then
    ACCEL=(-enable-kvm -cpu host)
else
    printf 'warning: KVM unavailable; using software emulation\n' >&2
    ACCEL=(-accel tcg -cpu max)
fi

mkdir -p "$ROOT_DIR/vm/logs"
LOG_FILE="$ROOT_DIR/vm/logs/base-boot-$(date +%Y%m%d-%H%M%S).log"

printf 'Starting Ubuntu base VM\n'
printf 'SSH: ssh -i ~/.ssh/cerynth_vm -p %s cerynth@127.0.0.1\n' "$SSH_PORT"
printf 'Exit QEMU: Ctrl+A, then X\n\n'

qemu-system-x86_64 \
  -name CerynthOS-base \
  -machine q35 \
  "${ACCEL[@]}" \
  -smp "$CPUS" \
  -m "$MEMORY" \
  -drive "file=$DISK,format=qcow2,if=virtio" \
  -drive "file=$SEED,format=raw,if=virtio,readonly=on" \
  -device virtio-net-pci,netdev=net0 \
  -netdev "user,id=net0,hostfwd=tcp::$SSH_PORT-:22" \
  -nographic \
  2>&1 | tee "$LOG_FILE"
EOF

chmod +x scripts/vm/run-base-vm.sh
```

---

## 12. First base VM boot

Start the Ubuntu VM:

```bash
./scripts/vm/run-base-vm.sh
```

Wait for:

```text
CERYNTH_BASE_VM_READY
```

The first boot can take several minutes because cloud-init updates packages and installs dependencies.

Open another terminal and connect:

```bash
ssh \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1
```

Inside the VM:

```bash
hostname
uname -a
systemctl is-system-running
cloud-init status --wait
```

Verify cloud-init:

```bash
test -f /var/log/cerynth-cloud-init-complete \
  && echo "Cloud-init complete" \
  || echo "Cloud-init incomplete"
```

Inspect the root filesystem:

```bash
findmnt /
lsblk -f
findmnt -no SOURCE /
findmnt -no UUID /
```

---

## 13. Stage custom kernel modules

On the host:

```bash
LINUX_SRC="$PWD/third_party/linux"
KERNEL_BUILD="$PWD/kernel/build"

KERNEL_RELEASE="$(
  make -s -C "$LINUX_SRC" \
    O="$KERNEL_BUILD" \
    kernelrelease
)"

rm -rf vm/modules/rootfs
mkdir -p vm/modules/rootfs
```

Install modules into the staging directory:

```bash
make -C "$LINUX_SRC" \
  O="$KERNEL_BUILD" \
  modules_install \
  INSTALL_MOD_PATH="$PWD/vm/modules/rootfs"
```

Verify:

```bash
ls -lah \
  "vm/modules/rootfs/lib/modules/$KERNEL_RELEASE"
```

Count modules:

```bash
find \
  "vm/modules/rootfs/lib/modules/$KERNEL_RELEASE" \
  -type f \
  -name '*.ko*' \
  | wc -l
```

---

## 14. Copy modules and kernel files into the VM

Create a temporary module directory in the guest:

```bash
ssh \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1 \
  "rm -rf /tmp/cerynth-modules && mkdir -p /tmp/cerynth-modules"
```

Copy modules:

```bash
rsync \
  -aH \
  --info=progress2 \
  -e "ssh -i $HOME/.ssh/cerynth_vm -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null" \
  "vm/modules/rootfs/lib/modules/$KERNEL_RELEASE/" \
  "cerynth@127.0.0.1:/tmp/cerynth-modules/"
```

Install modules inside the VM:

```bash
ssh \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1 \
  "sudo mkdir -p /lib/modules/$KERNEL_RELEASE && \
   sudo rsync -aH /tmp/cerynth-modules/ /lib/modules/$KERNEL_RELEASE/"
```

Copy the kernel image:

```bash
scp \
  -i "$HOME/.ssh/cerynth_vm" \
  -P 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  kernel/build/arch/x86/boot/bzImage \
  "cerynth@127.0.0.1:/tmp/vmlinuz-$KERNEL_RELEASE"
```

Copy the supporting files:

```bash
scp \
  -i "$HOME/.ssh/cerynth_vm" \
  -P 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  kernel/build/System.map \
  kernel/build/.config \
  cerynth@127.0.0.1:/tmp/
```

Install them:

```bash
ssh \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1 \
  "
    sudo install -m 0644 \
      /tmp/vmlinuz-$KERNEL_RELEASE \
      /boot/vmlinuz-$KERNEL_RELEASE

    sudo install -m 0644 \
      /tmp/System.map \
      /boot/System.map-$KERNEL_RELEASE

    sudo install -m 0644 \
      /tmp/.config \
      /boot/config-$KERNEL_RELEASE
  "
```

---

## 15. Generate a matching initramfs

Run from the host:

```bash
ssh \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1 \
  "
    sudo depmod -a '$KERNEL_RELEASE'
    sudo update-initramfs -c -k '$KERNEL_RELEASE'
  "
```

Verify:

```bash
ssh \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1 \
  "ls -lh /boot/initrd.img-$KERNEL_RELEASE"
```

Copy the initramfs back to the host:

```bash
scp \
  -i "$HOME/.ssh/cerynth_vm" \
  -P 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  "cerynth@127.0.0.1:/boot/initrd.img-$KERNEL_RELEASE" \
  "vm/initramfs/initrd.img-$KERNEL_RELEASE"
```

Verify:

```bash
ls -lh "vm/initramfs/initrd.img-$KERNEL_RELEASE"
```

---

## 16. Record the root filesystem

Run on the host while the base VM is running:

```bash
ROOT_DEVICE="$(
  ssh \
    -i "$HOME/.ssh/cerynth_vm" \
    -p 2222 \
    -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null \
    cerynth@127.0.0.1 \
    'findmnt -no SOURCE /'
)"

ROOT_UUID="$(
  ssh \
    -i "$HOME/.ssh/cerynth_vm" \
    -p 2222 \
    -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null \
    cerynth@127.0.0.1 \
    'findmnt -no UUID /'
)"

echo "Root device: $ROOT_DEVICE"
echo "Root UUID:   $ROOT_UUID"
```

Save it:

```bash
cat > vm/root-filesystem.env <<EOF
CERYNTH_ROOT_DEVICE=$ROOT_DEVICE
CERYNTH_ROOT_UUID=$ROOT_UUID
EOF
```

Inspect:

```bash
cat vm/root-filesystem.env
```

---

## 17. Shut down the base VM

```bash
ssh \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1 \
  "sudo poweroff"
```

Wait for QEMU to exit.

Never run two QEMU processes against the same overlay disk simultaneously.

---

## 18. Direct CerynthOS kernel launcher

Create `scripts/vm/run-cerynth-vm.sh`:

```bash
cat > scripts/vm/run-cerynth-vm.sh <<'EOF'
#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

LINUX_SRC="$ROOT_DIR/third_party/linux"
KERNEL_BUILD="$ROOT_DIR/kernel/build"
VM_DISK="${CERYNTH_VM_DISK:-$ROOT_DIR/vm/overlays/cerynth-dev.qcow2}"

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

command -v qemu-system-x86_64 >/dev/null ||
    die "qemu-system-x86_64 is not installed"

[[ -f "$LINUX_SRC/Makefile" ]] ||
    die "Linux source tree missing: $LINUX_SRC"

KERNEL_RELEASE="$(
    make -s -C "$LINUX_SRC" \
        O="$KERNEL_BUILD" \
        kernelrelease
)"

KERNEL_IMAGE="${CERYNTH_KERNEL_IMAGE:-$KERNEL_BUILD/arch/x86/boot/bzImage}"
INITRD_IMAGE="${CERYNTH_INITRD_IMAGE:-$ROOT_DIR/vm/initramfs/initrd.img-$KERNEL_RELEASE}"

MEMORY="${CERYNTH_VM_MEMORY:-6G}"
CPUS="${CERYNTH_VM_CPUS:-4}"
SSH_PORT="${CERYNTH_VM_SSH_PORT:-2222}"

[[ -f "$KERNEL_IMAGE" ]] ||
    die "kernel image missing: $KERNEL_IMAGE"

[[ -f "$INITRD_IMAGE" ]] ||
    die "initramfs missing: $INITRD_IMAGE"

[[ -f "$VM_DISK" ]] ||
    die "VM disk missing: $VM_DISK"

ROOT_SPEC=""

if [[ -n "${CERYNTH_ROOT_UUID:-}" ]]; then
    ROOT_SPEC="UUID=$CERYNTH_ROOT_UUID"
elif [[ -n "${CERYNTH_ROOT_DEVICE:-}" ]]; then
    ROOT_SPEC="$CERYNTH_ROOT_DEVICE"
elif [[ -f "$ROOT_DIR/vm/root-filesystem.env" ]]; then
    # shellcheck disable=SC1091
    source "$ROOT_DIR/vm/root-filesystem.env"

    if [[ -n "${CERYNTH_ROOT_UUID:-}" ]]; then
        ROOT_SPEC="UUID=$CERYNTH_ROOT_UUID"
    else
        ROOT_SPEC="${CERYNTH_ROOT_DEVICE:-/dev/vda1}"
    fi
else
    ROOT_SPEC="/dev/vda1"
fi

if [[ -r /dev/kvm && -w /dev/kvm ]]; then
    ACCEL=(-enable-kvm -cpu host)
    ACCEL_NAME="KVM"
else
    ACCEL=(-accel tcg -cpu max)
    ACCEL_NAME="TCG"
fi

mkdir -p "$ROOT_DIR/vm/logs"

TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
LOG_FILE="$ROOT_DIR/vm/logs/cerynth-boot-$TIMESTAMP.log"

KERNEL_CMDLINE="$(
    printf '%s' \
      "root=$ROOT_SPEC rw rootwait " \
      "console=ttyS0,115200n8 " \
      "loglevel=7 " \
      "systemd.show_status=1 " \
      "panic=10"
)"

printf '\nCerynthOS direct kernel boot\n'
printf '%-14s %s\n' "Kernel:" "$KERNEL_IMAGE"
printf '%-14s %s\n' "Release:" "$KERNEL_RELEASE"
printf '%-14s %s\n' "Initramfs:" "$INITRD_IMAGE"
printf '%-14s %s\n' "Disk:" "$VM_DISK"
printf '%-14s %s\n' "Root:" "$ROOT_SPEC"
printf '%-14s %s\n' "Acceleration:" "$ACCEL_NAME"
printf '%-14s %s\n' "CPUs:" "$CPUS"
printf '%-14s %s\n' "Memory:" "$MEMORY"
printf '%-14s localhost:%s\n' "SSH:" "$SSH_PORT"
printf '%-14s %s\n\n' "Boot log:" "$LOG_FILE"

printf 'Exit QEMU using Ctrl+A, then X\n\n'

qemu-system-x86_64 \
  -name CerynthOS-dev \
  -machine q35 \
  "${ACCEL[@]}" \
  -smp "$CPUS" \
  -m "$MEMORY" \
  -kernel "$KERNEL_IMAGE" \
  -initrd "$INITRD_IMAGE" \
  -drive "file=$VM_DISK,format=qcow2,if=virtio" \
  -device virtio-net-pci,netdev=net0 \
  -netdev "user,id=net0,hostfwd=tcp::$SSH_PORT-:22" \
  -append "$KERNEL_CMDLINE" \
  -no-reboot \
  -nographic \
  2>&1 | tee "$LOG_FILE"
EOF

chmod +x scripts/vm/run-cerynth-vm.sh
```

---

## 19. Boot CerynthOS

```bash
./scripts/vm/run-cerynth-vm.sh
```

Expected early output includes:

```text
Linux version <kernel-release>
```

Eventually the VM should reach a login prompt.

From another terminal:

```bash
ssh \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1
```

---

## 20. Install the boot marker

Create `packaging/systemd/cerynth-boot-marker.service`:

```ini
[Unit]
Description=CerynthOS successful boot marker
After=multi-user.target
ConditionPathIsReadWrite=/run

[Service]
Type=oneshot
ExecStart=/usr/bin/mkdir -p /run/cerynth
ExecStart=/usr/bin/sh -c 'printf "kernel=%s\ntimestamp=%s\n" "$$(uname -r)" "$$(date -u +%%FT%%TZ)" > /run/cerynth/boot-ok'
ExecStart=/usr/bin/sh -c 'echo CERYNTH_BOOT_OK > /dev/console'
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
```

Copy it:

```bash
scp \
  -i "$HOME/.ssh/cerynth_vm" \
  -P 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  packaging/systemd/cerynth-boot-marker.service \
  cerynth@127.0.0.1:/tmp/
```

Install and enable it:

```bash
ssh \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1 \
  "
    sudo install -m 0644 \
      /tmp/cerynth-boot-marker.service \
      /etc/systemd/system/cerynth-boot-marker.service

    sudo systemctl daemon-reload
    sudo systemctl enable --now cerynth-boot-marker.service
  "
```

Verify:

```bash
ssh \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1 \
  "cat /run/cerynth/boot-ok"
```

---

## 21. Final validation

Inside the CerynthOS VM:

```bash
uname -r
systemctl is-system-running
test -r /sys/kernel/btf/vmlinux && echo "BTF OK"
cat /sys/kernel/sched_ext/state
test -e /run/cerynth/boot-ok && echo "BOOT MARKER OK"
```

Known-good output:

```text
7.1.0
running
BTF OK
disabled
BOOT MARKER OK
```

Interpretation:

| Check | Meaning |
|---|---|
| `7.1.0` | The custom CerynthOS kernel is running |
| `running` | systemd reached a healthy userspace state |
| `BTF OK` | BTF is exposed for BPF and `sched_ext` tooling |
| `disabled` | `sched_ext` exists, but no custom scheduler is currently loaded |
| `BOOT MARKER OK` | The CerynthOS boot marker service completed |

---

## 22. Boot information collector

Create `scripts/vm/collect-boot-info.sh`:

```bash
cat > scripts/vm/collect-boot-info.sh <<'EOF'
#!/usr/bin/env bash
set -Eeuo pipefail

TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUTPUT_DIR="${1:-artifacts/boot-report/$TIMESTAMP}"

mkdir -p "$OUTPUT_DIR"

capture() {
    local name="$1"
    shift

    {
        printf '$'
        printf ' %q' "$@"
        printf '\n\n'
        "$@"
    } >"$OUTPUT_DIR/$name.txt" 2>&1 || true
}

capture uname uname -a
capture kernel-release uname -r
capture kernel-command-line cat /proc/cmdline
capture cpu lscpu
capture memory free -h
capture mounts findmnt
capture block-devices lsblk -f
capture modules lsmod
capture system-state systemctl is-system-running
capture failed-units systemctl --failed --no-pager
capture journal-warnings journalctl -b -p warning --no-pager
capture dmesg dmesg
capture dmesg-warnings dmesg --level=err,warn

if [[ -r /sys/kernel/btf/vmlinux ]]; then
    printf 'available\n' >"$OUTPUT_DIR/btf-status.txt"
else
    printf 'missing\n' >"$OUTPUT_DIR/btf-status.txt"
fi

if [[ -r /sys/kernel/sched_ext/state ]]; then
    capture sched-ext-state cat /sys/kernel/sched_ext/state
else
    printf 'unavailable\n' >"$OUTPUT_DIR/sched-ext-state.txt"
fi

if [[ -r "/boot/config-$(uname -r)" ]]; then
    cp "/boot/config-$(uname -r)" "$OUTPUT_DIR/kernel-config"
fi

cat >"$OUTPUT_DIR/summary.txt" <<SUMMARY
CerynthOS boot report
Timestamp: $TIMESTAMP
Hostname: $(hostname)
Kernel: $(uname -r)
System state: $(systemctl is-system-running 2>/dev/null || true)
BTF: $(cat "$OUTPUT_DIR/btf-status.txt")
sched_ext: $(cat "$OUTPUT_DIR/sched-ext-state.txt" 2>/dev/null || true)
SUMMARY

printf 'Boot report written to %s\n' "$OUTPUT_DIR"
EOF

chmod +x scripts/vm/collect-boot-info.sh
```

Copy it to the VM:

```bash
scp \
  -i "$HOME/.ssh/cerynth_vm" \
  -P 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  scripts/vm/collect-boot-info.sh \
  cerynth@127.0.0.1:/tmp/
```

Run it:

```bash
ssh \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1 \
  "
    mkdir -p ~/cerynth-artifacts
    sudo bash /tmp/collect-boot-info.sh ~/cerynth-artifacts/boot-report
    sudo chown -R cerynth:cerynth ~/cerynth-artifacts
  "
```

Copy the report back:

```bash
scp \
  -i "$HOME/.ssh/cerynth_vm" \
  -P 2222 \
  -r \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1:~/cerynth-artifacts/boot-report \
  artifacts/boot-report/
```

---

## 23. Reset the VM

Create `scripts/vm/reset-vm.sh`:

```bash
cat > scripts/vm/reset-vm.sh <<'EOF'
#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

BASE="$ROOT_DIR/vm/images/ubuntu-noble-base.img"
OVERLAY="$ROOT_DIR/vm/overlays/cerynth-dev.qcow2"

[[ -f "$BASE" ]] || {
    echo "Base image missing: $BASE" >&2
    exit 1
}

if pgrep -af qemu-system-x86_64 | grep -q "$OVERLAY"; then
    echo "Refusing to reset: VM appears to be running." >&2
    exit 1
fi

printf 'This will erase every change made inside the CerynthOS VM.\n'
read -r -p 'Type RESET to continue: ' confirmation

[[ "$confirmation" == "RESET" ]] || {
    echo "Reset cancelled."
    exit 0
}

rm -f "$OVERLAY"

qemu-img create \
  -f qcow2 \
  -F qcow2 \
  -b "$BASE" \
  "$OVERLAY"

echo "VM overlay reset successfully."
EOF

chmod +x scripts/vm/reset-vm.sh
```

Run:

```bash
./scripts/vm/reset-vm.sh
```

Resetting the overlay deletes all changes made inside the guest. Kernel modules and the generated initramfs must be installed again afterward.

---

## 24. Common failures

### KVM permission denied

```bash
ls -l /dev/kvm
groups
```

Add the user to the group:

```bash
sudo usermod -aG kvm "$USER"
```

Log out and back in.

---

### SSH connection refused

Check whether QEMU is still running:

```bash
pgrep -af qemu-system
```

Check the forwarded port:

```bash
nc -vz 127.0.0.1 2222
```

Use verbose SSH output:

```bash
ssh \
  -vv \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  cerynth@127.0.0.1
```

On the first boot, wait for cloud-init to finish.

---

### Image is locked

Another QEMU process is using the overlay:

```bash
pgrep -af qemu-system
```

Do not run the base VM and direct-kernel VM simultaneously.

---

### Kernel panic: unable to mount root filesystem

Check:

```bash
cat vm/root-filesystem.env
```

Try explicitly using `/dev/vda1`:

```bash
CERYNTH_ROOT_UUID="" \
CERYNTH_ROOT_DEVICE=/dev/vda1 \
./scripts/vm/run-cerynth-vm.sh
```

Check kernel support:

```bash
grep -E \
'^CONFIG_(VIRTIO|VIRTIO_PCI|VIRTIO_BLK|EXT4_FS)=' \
kernel/build/.config
```

Inspect the initramfs:

```bash
KERNEL_RELEASE="$(
  make -s -C "$PWD/third_party/linux" \
    O="$PWD/kernel/build" \
    kernelrelease
)"

lsinitramfs \
  "vm/initramfs/initrd.img-$KERNEL_RELEASE" \
  | grep -E 'virtio|ext4' \
  | head -30
```

---

### No serial output

Verify:

```bash
grep -E \
'^CONFIG_(SERIAL_8250|SERIAL_8250_CONSOLE)=' \
kernel/build/.config
```

Expected:

```text
CONFIG_SERIAL_8250=y
CONFIG_SERIAL_8250_CONSOLE=y
```

The direct launcher passes:

```text
console=ttyS0,115200n8
```

---

### `sched_ext` interface missing

Check the running kernel:

```bash
uname -r
```

Check the installed config:

```bash
grep CONFIG_SCHED_CLASS_EXT "/boot/config-$(uname -r)"
```

Expected:

```text
CONFIG_SCHED_CLASS_EXT=y
```

Also verify:

```bash
test -r /sys/kernel/btf/vmlinux && echo "BTF available"
```

---

### systemd reports `degraded`

Inspect failed units:

```bash
systemctl --failed
journalctl -b -p warning
```

Cloud images sometimes contain services that do not apply to the QEMU environment. Investigate each failed unit before treating `degraded` as a kernel failure.

---

## 25. Routine workflow after initial setup

Once everything is prepared, normal startup is:

```bash
./scripts/vm/run-cerynth-vm.sh
```

Connect:

```bash
ssh \
  -i "$HOME/.ssh/cerynth_vm" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  cerynth@127.0.0.1
```

Validate:

```bash
uname -r
systemctl is-system-running
test -r /sys/kernel/btf/vmlinux && echo "BTF OK"
cat /sys/kernel/sched_ext/state
test -e /run/cerynth/boot-ok && echo "BOOT MARKER OK"
```

Shut down:

```bash
sudo poweroff
```

Exit a QEMU `-nographic` session manually with:

```text
Ctrl+A, then X
```

---

## 26. Rebuilding and updating the custom kernel

After modifying and rebuilding the kernel:

```bash
make -C "$PWD/third_party/linux" \
  O="$PWD/kernel/build" \
  -j"$(nproc)" \
  2>&1 | tee kernel-build.log
```

Then repeat:

1. `modules_install` into `vm/modules/rootfs`
2. Boot the base VM
3. Copy the new modules and kernel files
4. Run `depmod`
5. Generate the new initramfs
6. Copy the new initramfs back to the host
7. Shut down the base VM
8. Run `scripts/vm/run-cerynth-vm.sh`

Do not reuse an initramfs generated for a different kernel release.

---

## 27. Git checklist

Commit:

```text
scripts/vm/run-base-vm.sh
scripts/vm/run-cerynth-vm.sh
scripts/vm/collect-boot-info.sh
scripts/vm/reset-vm.sh
packaging/systemd/cerynth-boot-marker.service
docs related to VM boot and recovery
.gitignore
```

Do not commit:

```text
vm/images/*.img
vm/images/*.qcow2
vm/overlays/*.qcow2
vm/cloud-init/*.iso
vm/logs/*
vm/modules/*
private SSH keys
large generated initramfs files unless intentionally distributed
```

Suggested commit:

```bash
git add \
  scripts/vm \
  packaging/systemd \
  docs \
  .gitignore

git commit -m "feat: add reproducible CerynthOS VM boot workflow"
```

---

## 28. Definition of done

The setup is complete when another developer can run:

```bash
./scripts/vm/run-cerynth-vm.sh
```

connect through SSH, and receive:

```text
7.1.0
running
BTF OK
disabled
BOOT MARKER OK
```

At that point:

- The custom CerynthOS kernel is running.
- Ubuntu userspace is healthy.
- BTF is available.
- `sched_ext` is ready for a scheduler.
- The VM boot is reproducible.
- Recovery is possible by recreating the overlay.
