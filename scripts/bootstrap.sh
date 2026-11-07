#!/usr/bin/env bash
set -Eeuo pipefail
if command -v pacman >/dev/null 2>&1; then
    sudo pacman -S --needed base-devel bc bison flex clang llvm openssl elfutils ncurses pahole pkgconf qemu-desktop qemu-img openssh rsync curl wget jq git shellcheck cloud-image-utils
elif command -v apt-get >/dev/null 2>&1; then
    sudo apt-get update
    sudo apt-get install -y build-essential bc bison flex clang llvm libssl-dev libelf-dev libncurses-dev dwarves pkg-config qemu-system-x86 qemu-utils cloud-image-utils openssh-client rsync curl wget jq git shellcheck
else
    printf 'Unsupported package manager; install host dependencies manually.\n' >&2
fi
if ! command -v rustup >/dev/null 2>&1; then curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; fi
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
rustup component add rustfmt clippy rust-src llvm-tools-preview
command -v just >/dev/null 2>&1 || cargo install just
printf 'Bootstrap complete. Next: just fetch-upstream && just doctor\n'
