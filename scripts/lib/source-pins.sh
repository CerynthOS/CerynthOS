#!/usr/bin/env bash
# Sourced by build scripts; never switch or discard an existing source tree.
require_pinned_source() {
    local source_dir="$1" pin_file="$2" expected actual
    expected="$(tr -d '[:space:]' < "$pin_file")"
    [[ "$expected" =~ ^[0-9a-f]{40}$ ]] || { printf 'error: invalid revision in %s\n' "$pin_file" >&2; return 1; }
    actual="$(git -C "$source_dir" rev-parse HEAD)" || return 1
    [[ "$actual" == "$expected" ]] || { printf 'error: %s is at %s; expected %s\n' "$source_dir" "$actual" "$expected" >&2; return 1; }
    [[ -z "$(git -C "$source_dir" status --porcelain --untracked-files=normal)" ]] || {
        printf 'error: preserve local changes in %s before a reproducible build\n' "$source_dir" >&2; return 1;
    }
}
