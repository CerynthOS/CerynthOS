#!/usr/bin/env bash
# Shared helpers for tests/workloads/*.sh

require_tool() {
  local tool="$1"
  local install_hint="$2"
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: required tool '${tool}' not found." >&2
    echo "install it with: ${install_hint}" >&2
    return 1
  fi
}
