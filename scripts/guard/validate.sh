#!/usr/bin/env bash
# Reproducible local Guard checks; saves evidence outside source directories.
set -euo pipefail
umask 077
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"
out="$(mktemp -d "${TMPDIR:-/tmp}/cerynth-guard-validation.XXXXXX")"
printf '%s\n' "Evidence: $out"
uname -a > "$out/host.txt"
git rev-parse HEAD > "$out/commit.txt"
cargo metadata --locked --format-version 1 > "$out/dependencies.json"
cargo build --locked -p cerynth-security -p cerynth-guardd --bins > "$out/build.log" 2>&1
cargo test --locked -p cerynth-security -p cerynth-guardd > "$out/tests.log" 2>&1
cargo clippy --locked -p cerynth-security -p cerynth-guardd --all-targets -- -A clippy::pedantic > "$out/clippy.log" 2>&1
GUARD_EXEC="$root/target/debug/cerynth-guard-exec" cargo test --locked -p cerynth-guardd --test managed_execution -- --ignored > "$out/managed-execution.log" 2>&1
GUARD_LAUNCHER="$root/target/debug/cerynth-guard-run" tests/security/sandbox-boundary.sh > "$out/boundary.log" 2>&1
printf '%s\n' 'All local checks passed. VM, provider and real harness results require separate evidence.' > "$out/result.txt"
cat "$out/result.txt"
