#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cargo_binary="${CARGO:-cargo}"
contract_tmp="$(mktemp -d)"
trap 'rm -rf -- "$contract_tmp"' EXIT
cd -- "$repo_root"

"$cargo_binary" run --locked --quiet -p cerynth-agent-api --bin cerynth-agent-schema \
    > "$contract_tmp/request.json"
"$cargo_binary" run --locked --quiet -p cerynth-agent-api --bin cerynth-agent-schema -- --session \
    > "$contract_tmp/session.json"
diff -u docs/contracts/schemas/agent-api-v1alpha1.schema.json "$contract_tmp/request.json"
diff -u docs/contracts/schemas/agent-session-v1alpha1.schema.json "$contract_tmp/session.json"
echo "Agent request and session schemas match their Rust contracts."
