# Person 3 — Week 1 runtime foundation

Based on master plan v2.0, immediate Week 1 tasks. Base: `origin/main` at
`e78be39` (November 10, 2026 checkout). This track adds independent experimental
components without changing scheduler behavior or the existing daemon contract.

| Deliverable | Implementation |
| --- | --- |
| Define AgentSession API | `crates/cerynth-agent-api`, [draft contract](../contracts/agent-session-v1alpha1.md) |
| Harness adapter skeleton | `HarnessAdapter` and `adapters/generic-pty`; unsupported operations fail |
| One local model endpoint | `crates/cerynth-model-api`, Ollama model discovery and bounded non-streaming chat |
| Minimal session registry | `services/cerynth-agentd`, private persistent registry and same-user Unix API/CLI |

This is a Week 1 endpoint experiment. It does not complete the Week 2 gate of
an actual coding agent running in a sandbox.

## Run

No root privileges, scheduler, or GPU are needed for the registry.

```bash
cargo build -p cerynth-agentd
export CERYNTH_AGENT_DIR="$HOME/.local/state/cerynth/agentd"
target/debug/cerynth-agentd serve --endpoint http://127.0.0.1:11434
```

In another terminal, with the same directory setting:

```bash
export CERYNTH_AGENT_DIR="$HOME/.local/state/cerynth/agentd"
target/debug/cerynth-agentd adapters
target/debug/cerynth-agentd models
target/debug/cerynth-agentd create \
  --name local-endpoint-probe --workspace "$PWD" --model YOUR_INSTALLED_MODEL
target/debug/cerynth-agentd list
target/debug/cerynth-agentd status SESSION_UUID
target/debug/cerynth-agentd infer SESSION_UUID < /path/to/prompt.txt
target/debug/cerynth-agentd status SESSION_UUID
```

The `create` output includes the UUID. Use it in place of `SESSION_UUID`.
`infer` reads stdin; input files can avoid storing prompts in shell history.
The response goes to stdout. A session accepts one inference attempt. Create a
new session to retry. `status` includes the full structured event timeline.

To cancel a pending inference from another terminal:

```bash
target/debug/cerynth-agentd cancel SESSION_UUID
```

Cancellation stops Cerynth's request; backend work may continue. Ctrl-C/SIGTERM
stops the daemon, records active failures, and removes its socket. Registry
state survives restarts. The prototype retains at most 128 sessions and has no
archive command; use a new private `--directory` for an independent experiment.
Do not delete the directory of a running daemon.

The default directory is `$XDG_STATE_HOME/cerynth/agentd`, falling back to
`$HOME/.local/state/cerynth/agentd`. Override with `--directory` or
`CERYNTH_AGENT_DIR`. Files are `sessions.json`, `registry.lock`, and `agentd.sock`.
Do not expose this directory to future untrusted agent processes.

## Real local-model smoke test

Start an operator-owned Ollama server configured for local-only operation:

```bash
OLLAMA_NO_CLOUD=1 ollama serve
```

If Ollama is already managed as a service, configure its environment there and
restart it instead of starting a competing server. Select an already installed
model from `models`; this prototype never downloads models. Run the commands
above and verify a completed session, assistant text, and backend-reported token
counters. CPU-only inference is supported by the backend, subject to model size
and request deadlines. No GPU availability or VRAM limit is claimed.

The implementation follows Ollama's documented [model discovery API](https://docs.ollama.com/api/tags)
and [chat API](https://docs.ollama.com/api/chat). The local-only deployment
setting is documented in the [Ollama FAQ](https://docs.ollama.com/faq#how-do-i-disable-ollamas-cloud-features).
Loopback transport alone cannot enforce backend egress policy.

## Automated verification

```bash
cargo fmt --all -- --check
cargo clippy -p cerynth-agent-api -p cerynth-model-api \
  -p cerynth-adapter-generic-pty -p cerynth-agentd --all-targets -- -D warnings
cargo test -p cerynth-agent-api -p cerynth-model-api \
  -p cerynth-adapter-generic-pty -p cerynth-agentd
cargo test --workspace
```

Tests use a real loopback HTTP fixture and Unix socket, private temporary state,
and the compiled CLI. They cover persistence, lock exclusion, restart recovery,
terminal-state protection, queue serialization, queued and active cancellation,
shutdown cleanup, malformed backend data, framing/version errors, empty inputs,
and absence of prompt/response text in stored events. No models, API keys,
internet inference, or GPU are required for these tests. A fixture is protocol
validation, not evidence of model quality, throughput, or real-agent success.

Validation on November 10, 2026, with Rust/Cargo 1.99.0:

- `cargo test --workspace --locked`: **196 passed, 0 failed, 3 ignored**,
  including 11 new runtime/API/backend tests.
- Strict Clippy for all four new crates and workspace formatting: passed.
- Restored the existing fake scheduler fixture's executable file mode; its
  eight control-plane integration tests pass.
- New HTTP dependencies were resolved to Rust 1.85-compatible versions in
  `Cargo.lock`; the 1.85 toolchain itself was not exercised.
- No Ollama server was listening at `127.0.0.1:11434` and no installed model
  manifests were found at the standard user/service locations. The real-model
  smoke test remains pending; automated inference results use the HTTP fixture.

## Next Person 3 tasks

Week 2: implement the first real harness adapter (OpenCode reference target),
connect Person 2's sandbox launcher and Person 1's cgroup handoff before claiming
managed execution, capture native tool events, then verify start/status/stop and
descendant cleanup on an actual useful coding task. Keep the PTY adapter's
capabilities empty until execution and cleanup are implemented and tested.

Week 3: second harness, reviewed parent/child session contract, and concurrent
inference observations. Later work covers GPU capability discovery, cooperative
VRAM accounting, backend lifecycle and cache experiments. Hard GPU guarantees
require actual supported enforcement mechanisms.
