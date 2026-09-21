#!/usr/bin/env bash

  set -Eeuo pipefail

  ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

  VM_DIR="$ROOT_DIR/vm"
  IMAGE_DIR="$VM_DIR/images"
  OVERLAY_DIR="$VM_DIR/overlays"
  GENERATED_DIR="$VM_DIR/generated"
  CLOUD_INIT_DIR="$VM_DIR/cloud-init"

  BASE_IMAGE="$IMAGE_DIR/ubuntu-noble-base.img"
  OVERLAY="$OVERLAY_DIR/cerynth-dev.qcow2"

  SSH_KEY="${CERYNTH_VM_SSH_KEY:-$HOME/.ssh/cerynth_vm}"
  USER_DATA_TEMPLATE="$CLOUD_INIT_DIR/user-data.template"
  GENERATED_USER_DATA="$GENERATED_DIR/user-data"
  SEED_ISO="$GENERATED_DIR/seed.iso"

  UBUNTU_IMAGE_URL="${CERYNTH_UBUNTU_IMAGE_URL:-https://cloud-images.ubuntu.com/noble/current/noble-
  server-cloudimg-amd64.img}"

  die() {
      printf 'error: %s\n' "$*" >&2
      exit 1
  }

  info() {
      printf '[cerynth-vm-setup] %s\n' "$*"
  }

  for tool in ssh-keygen qemu-img curl cloud-localds; do
      command -v "$tool" >/dev/null || die "required command missing: $tool"
  done

  mkdir -p \
      "$IMAGE_DIR" \
      "$OVERLAY_DIR" \
      "$GENERATED_DIR" \
      "$(dirname "$SSH_KEY")"

  if [[ ! -f "$SSH_KEY" ]]; then
      info "generating VM SSH key: $SSH_KEY"
      ssh-keygen -t ed25519 -f "$SSH_KEY" -C "cerynth-vm" -N ""
  else
      info "using existing VM SSH key: $SSH_KEY"
  fi

  [[ -f "$SSH_KEY.pub" ]] || die "public SSH key missing: $SSH_KEY.pub"
  [[ -f "$USER_DATA_TEMPLATE" ]] ||
      die "cloud-init template missing: $USER_DATA_TEMPLATE"

  PUBLIC_KEY="$(< "$SSH_KEY.pub")"

  info "generating cloud-init user-data"
  sed \
      "s|__CERYNTH_SSH_PUBLIC_KEY__|$PUBLIC_KEY|" \
      "$USER_DATA_TEMPLATE" \
      > "$GENERATED_USER_DATA"

  info "generating cloud-init seed"
  cloud-localds \
      "$SEED_ISO" \
      "$GENERATED_USER_DATA" \
      "$CLOUD_INIT_DIR/meta-data"

  if [[ ! -f "$BASE_IMAGE" ]]; then
      info "downloading Ubuntu Noble cloud image"
      curl -L --fail --progress-bar "$UBUNTU_IMAGE_URL" -o "$BASE_IMAGE"
      info "resizing base image to 30G"
      qemu-img resize "$BASE_IMAGE" 30G
  else
      info "base image already exists"
  fi

  if [[ ! -f "$OVERLAY" ]]; then
      info "creating writable VM overlay"
      qemu-img create -f qcow2 -F qcow2 -b "$BASE_IMAGE" "$OVERLAY"
  else
      info "VM overlay already exists"
  fi

printf '\nBase VM prepared.\n\n'
printf 'SSH key:    %s\n' "$SSH_KEY"
printf 'Base image: %s\n' "$BASE_IMAGE"
printf 'Overlay:    %s\n' "$OVERLAY"
printf 'Cloud-init: %s\n' "$SEED_ISO"
