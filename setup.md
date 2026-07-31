# CerynthOS

CerynthOS is a Linux-based, AI-first operating-system project focused on safe, adaptive resource management, programmable scheduling, system telemetry, policy enforcement, and user-personalised behaviour.

This repository currently contains the first development milestone:

- a Rust monorepo,
- shared CerynthOS libraries,
- a system daemon,
- a CLI,
- a `sched_ext` scheduler prototype,
- pinned upstream Linux and SCX source references,
- kernel build scripts,
- development checks and CI foundations.

> **Development status:** early bootstrap / first alpha infrastructure.  
> Do not install an experimental kernel or scheduler on your only bootable system.

---

## Contents

- [CerynthOS](#cerynthos)
  - [Contents](#contents)
  - [Project goals](#project-goals)
  - [Repository layout](#repository-layout)
  - [Supported development environment](#supported-development-environment)
  - [Clone the repository](#clone-the-repository)
  - [Install system dependencies](#install-system-dependencies)
  - [Configure virtualization](#configure-virtualization)
  - [Install Rust](#install-rust)
    - [Rust download fails with a DNS error](#rust-download-fails-with-a-dns-error)
  - [Install Rust development tools](#install-rust-development-tools)
  - [Verify the toolchain](#verify-the-toolchain)
  - [Build and test the Rust workspace](#build-and-test-the-rust-workspace)
    - [Strict Clippy documentation lints](#strict-clippy-documentation-lints)
  - [Fetch upstream Linux and SCX](#fetch-upstream-linux-and-scx)
    - [Manual Linux clone](#manual-linux-clone)
    - [Manual SCX clone](#manual-scx-clone)
  - [Build upstream SCX](#build-upstream-scx)
  - [Check host-kernel support](#check-host-kernel-support)
  - [Configure the CerynthOS kernel](#configure-the-cerynthos-kernel)
  - [Build the kernel](#build-the-kernel)
  - [Install the kernel on a test system](#install-the-kernel-on-a-test-system)
  - [Run an upstream scheduler safely](#run-an-upstream-scheduler-safely)
  - [Common development commands](#common-development-commands)
  - [Troubleshooting](#troubleshooting)
    - [`cargo init` says the package already exists](#cargo-init-says-the-package-already-exists)
    - [Workspace member manifest is missing](#workspace-member-manifest-is-missing)
    - [`cargo-nextest` fails to compile](#cargo-nextest-fails-to-compile)
    - [Missing `bpftool`](#missing-bpftool)
    - [SCX builds with warnings](#scx-builds-with-warnings)
    - [`git pull --rebase origin main` cannot find `main`](#git-pull---rebase-origin-main-cannot-find-main)
    - [GitHub blocks the push because a secret was detected](#github-blocks-the-push-because-a-secret-was-detected)
    - [Save terminal commands and output](#save-terminal-commands-and-output)
  - [Security and secrets](#security-and-secrets)
  - [Contribution workflow](#contribution-workflow)
  - [Current definition of done](#current-definition-of-done)
  - [License](#license)
  - [Maintainers](#maintainers)

---

## Project goals

The first technical goal is to establish a reproducible Linux development environment that can:

1. build the CerynthOS Rust workspace;
2. build an upstream `sched_ext` scheduler;
3. boot a compatible kernel with BPF, BTF, and `sched_ext`;
4. load and unload an upstream scheduler safely;
5. verify automatic fallback to the normal Linux scheduler;
6. provide a foundation for the Cerynth scheduler, daemon, CLI, telemetry, and policy layers.

The project should preserve these safety properties:

- reliable scheduler fallback;
- bounded policy decisions;
- starvation protection;
- observable system state;
- manual disable controls;
- recovery through a known-good distro kernel.

---

## Repository layout

```text
CerynthOS/
├── apps/
│   └── cerynth-control/          # Future desktop/control application
├── cli/
│   └── cerynthctl/               # Command-line control utility
├── crates/
│   ├── cerynth-common/           # Shared types and utilities
│   ├── cerynth-config/           # Configuration loading and validation
│   ├── cerynth-ipc/              # IPC protocol and transport abstractions
│   ├── cerynth-policy/           # Policy definitions and enforcement
│   └── cerynth-telemetry/        # Metrics and observability
├── services/
│   └── cerynthd/                 # Main CerynthOS system daemon
├── schedulers/
│   ├── bpf/                      # Cerynth BPF scheduler components
│   ├── cerynth-scx/              # Rust scheduler prototype
│   ├── SCX_VERSION               # Pinned upstream SCX version
│   └── SCX_COMMIT                # Pinned upstream SCX commit
├── kernel/
│   ├── config/                   # Kernel configuration inputs
│   ├── patches/                  # CerynthOS kernel patches
│   ├── VERSION                   # Pinned Linux version
│   └── COMMIT                    # Pinned Linux commit
├── distro/                       # Distribution build configuration
├── packaging/                    # Debian and systemd packaging
├── scripts/                      # Bootstrap, doctor, kernel, and SCX scripts
├── tests/
│   ├── integration/
│   └── workloads/
├── benchmarks/
├── models/
├── data/
│   └── samples/
├── docs/
└── third_party/                  # Locally cloned Linux and SCX trees
```

The `third_party/linux` and `third_party/scx` directories are intentionally ignored by Git. They are fetched locally and pinned through version and commit files stored in the repository.

---

## Supported development environment

The current setup targets:

- Ubuntu 24.04 or another recent Debian-based distribution;
- x86-64;
- a machine with hardware virtualization support;
- a recent Rust stable toolchain;
- Clang/LLVM;
- QEMU/KVM;
- Linux kernel and eBPF development tools.

A virtual machine or disposable test machine is strongly recommended for custom-kernel and scheduler testing.

---

## Clone the repository

Using SSH:

```bash
git clone git@github.com:CerynthOS/CerynthOS.git
cd CerynthOS
```

Using HTTPS:

```bash
git clone https://github.com/CerynthOS/CerynthOS.git
cd CerynthOS
```

Inspect the repository:

```bash
git remote -v
git status
git branch --show-current
```

If the repository does not yet have a local `main` branch:

```bash
git switch -c main
```

If `main` already exists:

```bash
git switch main
```

---

## Install system dependencies

On Ubuntu or Debian:

```bash
sudo apt update

sudo apt install -y \
  build-essential \
  git \
  curl \
  wget \
  jq \
  rsync \
  unzip \
  tar \
  xz-utils \
  zstd \
  ca-certificates \
  gnupg \
  pkg-config \
  cmake \
  ninja-build \
  meson \
  clang \
  llvm \
  lld \
  gcc \
  g++ \
  make \
  flex \
  bison \
  bc \
  cpio \
  kmod \
  dwarves \
  pahole \
  libelf-dev \
  libssl-dev \
  libdw-dev \
  libudev-dev \
  libz-dev \
  libzstd-dev \
  libcap-dev \
  libseccomp-dev \
  libbpf-dev \
  linux-tools-common \
  linux-tools-generic \
  qemu-system-x86 \
  qemu-utils \
  ovmf \
  virt-manager \
  libvirt-daemon-system \
  libvirt-clients \
  python3 \
  python3-venv \
  python3-pip \
  shellcheck
```

Some distributions split kernel tools into version-specific packages. If `bpftool` reports that tools are missing for the running kernel, see [Missing `bpftool`](#missing-bpftool).

---

## Configure virtualization

Add your user to the required groups:

```bash
sudo usermod -aG libvirt,kvm "$USER"
```

Log out and back in before continuing.

Verify access:

```bash
ls -l /dev/kvm
virsh list --all
qemu-system-x86_64 --version
```

If `/dev/kvm` is unavailable, ensure virtualization is enabled in firmware and that the KVM kernel modules are loaded.

---

## Install Rust

Install Rust with `rustup`:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Choose the standard installation, then load Cargo into the current shell:

```bash
source "$HOME/.cargo/env"
```

Install and select stable Rust:

```bash
rustup toolchain install stable
rustup default stable
```

Install the required components:

```bash
rustup component add \
  rustfmt \
  clippy \
  rust-src \
  llvm-tools-preview
```

### Rust download fails with a DNS error

Check basic connectivity:

```bash
ping -c 3 1.1.1.1
ping -c 3 google.com
```

If the IP address works but the hostname fails:

```bash
sudo systemctl restart systemd-resolved
sudo resolvectl flush-caches
resolvectl status
```

Temporarily configure public DNS resolvers on the default network interface:

```bash
IFACE="$(ip route | awk '/default/ {print $5; exit}')"

sudo resolvectl dns "$IFACE" 1.1.1.1 8.8.8.8
sudo resolvectl domain "$IFACE" '~.'
```

Verify DNS and retry:

```bash
getent hosts static.rust-lang.org
curl -I https://static.rust-lang.org
rustup update stable
```

Also check for stale proxy environment variables:

```bash
env | grep -i proxy
```

Clear invalid proxies when necessary:

```bash
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY all_proxy
```

---

## Install Rust development tools

Install the common workspace tools:

```bash
cargo install \
  cargo-audit \
  cargo-deny \
  cargo-watch \
  just
```

Install `cargo-nextest` with its locked dependency set:

```bash
cargo install --locked cargo-nextest
```

If compiling `cargo-nextest` from source fails, use the upstream prebuilt binary installer:

```bash
curl -LsSf https://get.nexte.st/latest/linux \
  | tar zxf - -C "$HOME/.cargo/bin"
```

Verify the tools:

```bash
cargo nextest --version
cargo audit --version
cargo deny --version
cargo watch --version
just --version
```

---

## Verify the toolchain

```bash
rustc --version
cargo --version
clang --version
bpftool version
pahole --version
qemu-system-x86_64 --version
```

A successful local setup used:

- stable Rust;
- Cargo;
- Clang 18;
- `pahole` 1.25;
- QEMU/KVM;
- upstream SCX built successfully in release mode.

Exact versions may differ as long as they satisfy the currently pinned upstream projects.

---

## Build and test the Rust workspace

Fetch dependencies and check every package:

```bash
cargo metadata --format-version 1 --no-deps
cargo check --workspace
```

Run the full quality gate:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Using `just`:

```bash
just check
just fmt
just lint
just test
```

Or run the combined CI-style check:

```bash
just ci
```

### Strict Clippy documentation lints

This repository treats warnings as errors during linting. Markdown-like identifiers in Rust documentation should use backticks:

```rust
//! Core library module for this `CerynthOS` component.
```

Avoid:

```rust
//! Core library module for this CerynthOS component.
```

Clippy may report the second form through `clippy::doc_markdown`.

---

## Fetch upstream Linux and SCX

The repository should provide:

```bash
./scripts/fetch-upstream.sh
```

Run:

```bash
./scripts/fetch-upstream.sh
```

Or:

```bash
just fetch-upstream
```

### Manual Linux clone

```bash
git clone \
  --filter=blob:none \
  --no-checkout \
  https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git \
  third_party/linux

git -C third_party/linux fetch --tags --force
```

List recent kernel tags:

```bash
git -C third_party/linux tag \
  --sort=-version:refname \
  | grep -E '^v[0-9]+\.[0-9]+(\.[0-9]+)?$' \
  | head -20
```

Check out the version recorded by the repository:

```bash
CERYNTH_KERNEL_TAG="$(tr -d '[:space:]' < kernel/VERSION)"
git -C third_party/linux checkout "$CERYNTH_KERNEL_TAG"
```

Verify the exact commit:

```bash
git -C third_party/linux rev-parse HEAD
cat kernel/COMMIT
```

### Manual SCX clone

```bash
git clone \
  --filter=blob:none \
  https://github.com/sched-ext/scx.git \
  third_party/scx
```

Check out the pinned version:

```bash
CERYNTH_SCX_TAG="$(tr -d '[:space:]' < schedulers/SCX_VERSION)"

git -C third_party/scx fetch --tags
git -C third_party/scx checkout "$CERYNTH_SCX_TAG"
```

Verify:

```bash
git -C third_party/scx rev-parse HEAD
cat schedulers/SCX_COMMIT
```

---

## Build upstream SCX

Build all schedulers:

```bash
cd third_party/scx

BPF_CLANG="$(command -v clang)" \
cargo build --release

cd ../..
```

A successful build ends with output similar to:

```text
Finished `release` profile [optimized] target(s)
```

Warnings such as an unused manifest key or an `__arena` macro redefinition may appear in some pinned upstream versions. They are non-fatal when the release build completes successfully.

List generated scheduler binaries:

```bash
find third_party/scx/target/release \
  -maxdepth 1 \
  -type f \
  -executable \
  -name 'scx_*' \
  -printf '%f\n' \
  | sort
```

Build only Rustland when supported by the pinned release:

```bash
cargo build \
  --manifest-path third_party/scx/Cargo.toml \
  --release \
  -p scx_rustland
```

Inspect a binary:

```bash
third_party/scx/target/release/scx_rustland --help
file third_party/scx/target/release/scx_rustland
ldd third_party/scx/target/release/scx_rustland
```

---

## Check host-kernel support

Inspect the running kernel:

```bash
uname -a
uname -r
```

Check the `sched_ext` interface:

```bash
if [[ -e /sys/kernel/sched_ext/state ]]; then
  echo "sched_ext interface detected"
  cat /sys/kernel/sched_ext/state
else
  echo "sched_ext interface not available"
fi
```

Inspect the kernel configuration:

```bash
KCONFIG="/boot/config-$(uname -r)"

if [[ -r "$KCONFIG" ]]; then
  grep -E \
    'CONFIG_(SCHED_CLASS_EXT|BPF|BPF_SYSCALL|BPF_JIT|DEBUG_INFO_BTF)=' \
    "$KCONFIG"
else
  echo "Cannot read $KCONFIG"
fi
```

Check BTF:

```bash
test -r /sys/kernel/btf/vmlinux \
  && echo "BTF available" \
  || echo "BTF missing"
```

Run the project doctor:

```bash
./scripts/doctor.sh
```

Or:

```bash
just doctor
```

A host kernel that lacks `sched_ext` can still build the Rust workspace and often build upstream SCX, but it cannot load a `sched_ext` scheduler at runtime.

---

## Configure the CerynthOS kernel

Copy the current distro kernel configuration as a starting point:

```bash
cp "/boot/config-$(uname -r)" kernel/config/base.config
```

Configure the build:

```bash
./scripts/configure-kernel.sh
```

Or:

```bash
just kernel-config
```

The script should ensure the required kernel options are enabled, including the relevant BPF, BTF, and scheduler-extension options.

To merge upstream SCX recommendations:

```bash
cat third_party/scx/kernel.config >> kernel/build/.config

make -C third_party/linux \
  O="$PWD/kernel/build" \
  olddefconfig
```

Inspect the result:

```bash
grep -E \
  'CONFIG_(SCHED_CLASS_EXT|BPF|BPF_SYSCALL|BPF_JIT|DEBUG_INFO_BTF)=' \
  kernel/build/.config
```

---

## Build the kernel

Run:

```bash
./scripts/build-kernel.sh
```

Or:

```bash
just kernel-build
```

Override parallelism when required:

```bash
JOBS=8 ./scripts/build-kernel.sh
```

The expected x86-64 kernel image is:

```text
kernel/build/arch/x86/boot/bzImage
```

Verify:

```bash
ls -lh kernel/build/arch/x86/boot/bzImage
```

Kernel builds are resource-intensive. Ensure the machine has sufficient disk space and memory before starting.

---

## Install the kernel on a test system

> **Warning:** Use a VM or disposable test system first. Keep the distro kernel installed and bootable.

Determine the release name:

```bash
KERNEL_RELEASE="$(
  make -sC third_party/linux \
    O="$PWD/kernel/build" \
    kernelrelease
)"

echo "$KERNEL_RELEASE"
```

Install modules:

```bash
sudo make -C third_party/linux \
  O="$PWD/kernel/build" \
  modules_install
```

Install the kernel:

```bash
sudo make -C third_party/linux \
  O="$PWD/kernel/build" \
  install
```

Update initramfs and GRUB:

```bash
sudo update-initramfs -c -k "$KERNEL_RELEASE"
sudo update-grub
```

Reboot:

```bash
sudo reboot
```

After boot:

```bash
uname -r
cat /sys/kernel/sched_ext/state
```

Never remove the known-good distro kernel until the custom kernel has been tested thoroughly.

---

## Run an upstream scheduler safely

Before running any scheduler:

```bash
test -e /sys/kernel/sched_ext/state
cat /sys/kernel/sched_ext/state
```

Locate Rustland:

```bash
find third_party/scx/target/release \
  -maxdepth 1 \
  -type f \
  -name 'scx_rustland*' \
  -executable
```

Run it in a dedicated test environment:

```bash
sudo third_party/scx/target/release/scx_rustland
```

In another terminal:

```bash
cat /sys/kernel/sched_ext/state
cat /sys/kernel/sched_ext/root/ops 2>/dev/null || true
cat /sys/kernel/sched_ext/enable_seq
```

Stop the scheduler with `Ctrl+C`, then verify fallback:

```bash
cat /sys/kernel/sched_ext/state
```

The expected state after the scheduler exits is:

```text
disabled
```

Do not test an experimental scheduler during critical work on your main desktop.

---

## Common development commands

List project tasks:

```bash
just
```

Workspace checks:

```bash
just check
just fmt
just lint
just test
just audit
just ci
```

Build operations:

```bash
just scx-build
just kernel-config
just kernel-build
```

Environment diagnostics:

```bash
just doctor
```

Watch Rust sources:

```bash
cargo watch -x 'check --workspace'
```

Run tests through Nextest:

```bash
cargo nextest run --workspace
```

Audit dependencies:

```bash
cargo audit
cargo deny check
```

---

## Troubleshooting

### `cargo init` says the package already exists

Example:

```text
error: `cargo init` cannot be run on existing Cargo packages
```

This means the target directory already contains a valid Cargo package. Do not initialise it again.

Verify:

```bash
find cli crates services schedulers -name Cargo.toml -print
cargo metadata --format-version 1 --no-deps
cargo check --workspace
```

### Workspace member manifest is missing

Example:

```text
failed to load manifest for workspace member ...
No such file or directory
```

The root workspace references a package whose `Cargo.toml` does not exist yet.

Create every missing package, then run:

```bash
cargo metadata --format-version 1 --no-deps
cargo check --workspace
```

When bootstrapping a workspace manually, warnings can occur while some members exist and others have not yet been initialised. The workspace becomes valid once all member manifests are present.

### `cargo-nextest` fails to compile

Retry with the lockfile:

```bash
cargo install --locked cargo-nextest
```

Or install the prebuilt binary:

```bash
curl -LsSf https://get.nexte.st/latest/linux \
  | tar zxf - -C "$HOME/.cargo/bin"
```

### Missing `bpftool`

Example:

```text
WARNING: bpftool not found for kernel ...
```

Try:

```bash
sudo apt update
sudo apt install linux-tools-common linux-tools-generic
```

If the exact package for the running kernel exists:

```bash
sudo apt install "linux-tools-$(uname -r)"
```

Some older or no-longer-published kernel versions do not have matching packages in the currently configured repositories. In that situation:

1. install the current generic kernel and tools packages;
2. upgrade the system;
3. reboot into the matching kernel;
4. verify again with `bpftool version`.

Do not install `linux-cloud-tools` unless the project or environment specifically requires it.

### SCX builds with warnings

Warnings in upstream code are not necessarily project failures. Treat the build as successful only when Cargo prints:

```text
Finished `release` profile [optimized] target(s)
```

Record unexpected warnings in an issue when they affect reproducibility or runtime behaviour.

### `git pull --rebase origin main` cannot find `main`

Example:

```text
fatal: couldn't find remote ref main
```

This commonly means the remote repository has no `main` branch yet.

Verify:

```bash
git remote -v
git ls-remote --heads origin
```

If this is the first push:

```bash
git push -u origin main
```

### GitHub blocks the push because a secret was detected

Do not bypass push protection for a real credential.

Immediately revoke or rotate the exposed credential in the provider dashboard.

Remove local terminal logs from Git:

```bash
git rm --cached \
  terminal-history.txt \
  terminal-history-*.txt \
  terminal-session*.log 2>/dev/null || true
```

Add permanent ignore rules:

```bash
cat >> .gitignore <<'EOF'

# Local terminal records may contain credentials
terminal-history*.txt
terminal-session*.log
EOF
```

Amend the unpushed commit:

```bash
git add .gitignore
git commit --amend --no-edit
```

Search the current commit:

```bash
git grep -nE \
  'hf_[A-Za-z0-9]{20,}|gh[pousr]_[A-Za-z0-9_]{20,}' \
  HEAD \
  || echo "No obvious token found in HEAD"
```

If the credential appears in older local commits, rewrite the affected history before pushing. Never place real tokens in shell commands, committed `.env` files, examples, screenshots, terminal-history exports, or issue descriptions.

### Save terminal commands and output

To save future commands and terminal output:

```bash
script -a "terminal-session-$(date +%Y-%m-%d_%H-%M-%S).log"
```

Exit the recording session with:

```bash
exit
```

For a single command:

```bash
cargo build --release 2>&1 | tee cargo-build.log
```

Bash history records commands, not previous command output:

```bash
history > "terminal-history-$(date +%Y-%m-%d_%H-%M-%S).txt"
```

Terminal-history and session files must remain untracked because they can contain credentials.

---

## Security and secrets

Never commit:

- API tokens;
- SSH private keys;
- cloud credentials;
- `.env` files containing secrets;
- exported terminal history;
- terminal session recordings;
- production configuration;
- model-provider access tokens.

Recommended `.gitignore` entries:

```gitignore
# Logs and terminal records
*.log
terminal-history*.txt
terminal-session*.log

# Environment files
.env
.env.*
!.env.example

# Credentials
*.pem
*.key
credentials.json
```

Before pushing:

```bash
git status
git diff --cached --check
git grep -nE \
  'hf_[A-Za-z0-9]{20,}|gh[pousr]_[A-Za-z0-9_]{20,}' \
  --cached \
  || true
```

Use a secret manager, OS keyring, or provider CLI login flow instead of placing tokens directly into shell history.

---

## Contribution workflow

Create a branch:

```bash
git switch -c feat/<short-description>
```

Before committing:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
git diff --check
```

Commit:

```bash
git add .
git commit -m "feat: describe the change"
```

Bring the branch up to date:

```bash
git fetch origin
git rebase origin/main
```

Push:

```bash
git push -u origin HEAD
```

Open a pull request and include:

- the problem being solved;
- the implementation approach;
- test evidence;
- safety or rollback considerations;
- kernel/runtime requirements;
- known limitations.

---

## Current definition of done

The first systems milestone is complete when the following sequence works reproducibly:

```text
Clone repository
        ↓
Install dependencies
        ↓
Build and test Rust workspace
        ↓
Fetch pinned Linux and SCX sources
        ↓
Build upstream SCX
        ↓
Boot a compatible test kernel
        ↓
Load an upstream scheduler
        ↓
Observe sched_ext as enabled
        ↓
Stop the scheduler
        ↓
Verify safe fallback to disabled
```

The immediate technical checkpoint is:

```bash
sudo third_party/scx/target/release/scx_rustland
```

followed in another terminal by:

```bash
cat /sys/kernel/sched_ext/state
```

The state should become enabled while the scheduler is running and return to disabled after it exits.

Only after this path is stable should the project proceed to deeper scheduler policy work, AI-assisted optimisation, desktop integration, distro images, or production deployment.

---

## License

Apache-2.0. See `LICENSE` when present.

## Maintainers

The CerynthOS Contributors.