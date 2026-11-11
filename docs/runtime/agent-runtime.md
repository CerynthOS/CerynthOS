# Experimental agent runtime

The runtime launches owned agents, records sessions and artifacts, and routes
supported inference through per-session grants. It is a development component,
not the complete CerynthOS public alpha. The weekly plan is unchanged; acceptance
of all 112 tasks is tracked in [the ledger](../roadmap/person3-tasks.md).

## Build and prerequisites

```bash
cargo build --locked -p cerynth-agentd -p cerynth-inferenced -p cerynth-resources
cargo test --locked --workspace
scripts/check-agent-contracts.sh
```

Use Linux with `/proc`, cgroups v2, a working systemd user manager, and Bubblewrap
at `/usr/bin/bwrap`, Git, and util-linux `setpriv` with `--pdeathsig` support.
Build/install `cerynth-worktree-checkout` beside `cerynth-agentd`; Cargo builds both
service binaries by default, including `cerynth-harness-bridge`. A GPU is optional.
The default namespace profile denies network access. Ordinary native launches need
explicit `network: host` for localhost inference; that includes external destinations.
The opt-in native Guard mode below provides a fixed inference gateway while
keeping the agent's host networking denied.

Install a supported harness separately. Tested development versions are OpenCode
1.19.5, Hermes 0.23.1, Codex 0.167.0, and Ollama 0.35.0. Claude Code 2.1.312
installation detection was checked; cloud execution was not. OpenCode 2.x
configuration is not interchangeable with this pinned 1.x adapter.

Owner environment overrides select absolute executable paths:
`CERYNTH_OPENCODE_BINARY`, `CERYNTH_HERMES_BINARY`, `CERYNTH_CODEX_BINARY`,
`CERYNTH_CLAUDE_BINARY`. Hermes needs a pinned source installation with its `.venv`;
`CERYNTH_HERMES_INSTALL` optionally selects that tree. It is mounted read-only.
No harness or model is downloaded by the supervisor. `runtime.transport` defaults
to `cli_jsonl`. OpenCode additionally supports `open_code_http` and
`open_code_acp`, using the adjacent bridge binary. HTTP owns a password-protected
loopback OpenCode server and collects tool/history events after the turn. ACP
streams v1 updates, advertises no host filesystem/terminal/MCP capabilities, and
denies ungranted client operations. These transports do not advertise native
pause/resume or automatic subagent mapping.

## Start local inference and the services

Start an operator-owned Ollama server with `OLLAMA_NO_CLOUD=1`. Download a model
explicitly using Ollama before launching an agent. The host test used
`qwen2.5:3b`; small models can fail coding tasks despite a successful CLI exit.

In one terminal, start the coordinator:

```bash
target/debug/cerynth-inferenced serve --concurrency 1 --listen 127.0.0.1:11435
```

Its state defaults to `$XDG_STATE_HOME/cerynth/inferenced`, otherwise
`~/.local/state/cerynth/inferenced`. Register the existing local endpoint using a
JSON operation file:

```json
{"type":"configure","endpoint":{"kind":"ollama","id":"local","url":"http://127.0.0.1:11434"}}
```

```bash
target/debug/cerynth-inferenced request --admin endpoint.json
target/debug/cerynth-inferenced health
```

In another terminal, connect the agent supervisor to the coordinator:

```bash
export CERYNTH_INFERENCE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/cerynth/inferenced"
export CERYNTH_INFERENCE_HTTP=http://127.0.0.1:11435
target/debug/cerynth-agentd serve
```

Both variables must be set together. The agent directory defaults to the adjacent
`cerynth/agentd` state directory. Directories must be owned by the current user
with mode 0700. Sockets and state files are private. Run one owner daemon per
directory; a lock rejects competing instances.

`CERYNTH_PROJECT_INFERENCE_LIMIT` is an owner-only setting (1–32, default 4)
bounding native harness requests across a project, in addition to session and
ancestor limits. It cannot be raised by an agent request.

## Create and run a session

Save a session specification with an existing absolute workspace path:

```json
{
  "name": "local-coding",
  "harness": "open_code",
  "workspace": "/absolute/path/to/project",
  "model": {
    "name": "qwen2.5:3b",
    "endpoint_id": "local",
    "disable_thinking": true
  },
  "runtime": {
    "project": "sample",
    "network": "host",
    "budget": {"timeout_seconds": 180}
  }
}
```

```bash
target/debug/cerynth-agentd create-spec session.json
target/debug/cerynth-agentd adapters
target/debug/cerynth-agentd list
```

Use the returned UUID in the following commands. `harness-start` reads the task
from stdin. Retain its operation ID when retrying an acknowledgement; a changed
operation under the same ID is rejected. Use the JSON `request` command to
supply a stable operation ID if needed.

```bash
target/debug/cerynth-agentd harness-start SESSION_UUID < task.txt
target/debug/cerynth-agentd status SESSION_UUID
target/debug/cerynth-agentd events SESSION_UUID --after 0
target/debug/cerynth-agentd output SESSION_UUID --offset 0
target/debug/cerynth-agentd processes SESSION_UUID
target/debug/cerynth-agentd stop SESSION_UUID
target/debug/cerynth-inferenced metrics
```

The supervisor mints a narrowly scoped inference token, configures the harness's
compatible endpoint, and revokes the token on termination. It never passes the
coordinator's administrative socket into the namespace. Host credentials and
user configuration are not inherited. A detected host login does not imply the
managed harness has that credential. Claude's launch recipe consequently does
not provide working authenticated cloud execution.

Generic process/PTY sessions use `harness: generic_pty`, then `start UUID --pty
/absolute/program arguments...` and `input UUID` from stdin. The default
Bubblewrap/systemd boundary still applies. Development-only unisolated sessions
need both an explicit spec and `serve --allow-unisolated`; these are labelled
`process_group`/`integrated` and have weaker protection.

## Advanced operations and persistence

`cerynth-agentd request FILE` accepts a single operation without the version
envelope. Available operations include delegation, graphs, authorized sharing,
messages, phase reports, worktrees, artifacts, GPU reservations, observation,
resource requests, experimental A2A, and archival. The generated
[JSON Schema](../contracts/schemas/agent-api-v1alpha1.schema.json) is authoritative
for field names. Native clients use the versioned envelope over Unix IPC.

The owner grants `task.delegate`, `message.share`, or `gpu.execute` capabilities
explicitly. Child budgets and grants cannot exceed their parent. Parent exit
cancels descendants. Message sharing requires a separate owner authorization.
Creating a child does not automatically start its harness.

Owner operation `{"type":"retry","id":"SESSION_UUID","acknowledge_prior_effects":true}`
creates an unstarted retry session with the original specification and durable
lineage. It permits three retries, only for failed/cancelled owned sessions with
known operation outcomes and complete cleanup. Repeating the request returns the
same new UUID, including after restart. Unknown outcomes and observed processes
are rejected. This does not repeat tools or undo prior effects; the owner must
inspect the old work before explicitly launching the new attempt. Prepare a new
worktree when needed; a previous worktree is not silently reused.

Use `worktree UUID` before launch for a separate Git workspace. Registration
disables hooks/fsmonitor/fetching; a separately sandboxed checkout helper disables
clean/smudge/process filters, denies networking and limits writable Git metadata
to that worktree. Branches and worktrees are retained. Repository-defined checkout
filters (including Git LFS smudge) will not populate derived content automatically.
Prepare required assets explicitly. Active overlapping writers prevent preparation;
an interrupted preparation becomes failed rather than silently using the source.
Git operations needing the original repository's private
metadata may fail inside the isolated profile; do not expose the whole host
checkout as a workaround. Artifacts are bounded, hashed snapshots opened beneath
the workspace without symlinks. Artifact labels do not prove tests passed.

`tools/collect_task_evidence.py` accepts a terminal managed session, one relative
candidate path, an explicit owner baseline and an owner-selected test LaunchRequest.
It snapshots the child output/candidate, computes a text diff without running Git
hooks or filters, and runs the checker in a fresh network-denied managed session.
The test receipt records the actual exit, launch fingerprint, boundary, source
task/parent and whether the checker changed the candidate. Passing means only
that the owner's specified assertions exited zero; failed results are retained.

Archive a terminal session only after cleanup completes:

```json
{"type":"archive","id":"SESSION_UUID"}
```

Status and launch-operation deduplication survive archival. Archive children
before parents. There are 128 active registry slots, a 4 MiB registry bound,
4096 archived operation IDs, 1 MiB raw output per session, and 256 MiB artifact
storage. Archival retains metadata/output/artifacts; it does not remove branches.
Operators should retain/export evidence rather than deleting state to bypass
an unknown operation outcome.

On restart, owned unfinished work becomes failed, old process identities are
checked, and leftover groups/scopes are cleaned before new clients are accepted.
Terminal sessions with pending cleanup are recovered too. Started operations
with no known completion become unknown. No irreversible action is automatically
replayed or rolled back. Observed processes are never killed by the supervisor.
`cleanup UUID` retries failed owned-scope or Guard authorization cleanup; archival
remains blocked until it succeeds. Managed payloads use a kernel parent-death
signal plus an original-parent check to prevent late launch after daemon death.

## Optional Guard integration

Person 2's `feat/guard-sandbox` protocol at revision
`e592fa47797179ad24a99fc76143be48191a7649` is supported for offline generic
commands. Run its Guard service separately and set the trusted owner environment
`CERYNTH_GUARD_SOCKET` to its absolute control socket. Add to a `generic_pty` spec:

```json
{"runtime":{"guard":{"capabilities":[{"kind":"fs.read","target":"workspace"},{"kind":"fs.write","target":"workspace"}]}}}
```

This is a fragment to merge into a complete spec. Guard requires isolation,
systemd limits and denied networking. Select the final workspace before creating
the session. The runtime records the Guard profile digest and labels the actual
boundary `guard_systemd`. Registration, launch and cleanup failures never fall
back to the ordinary profile. Child authority is checked by Guard as well as local
budget validation; authorizations end only after process cleanup, children first.
OpenCode roots can opt into a private native inference gateway, without changing
Person 2's pinned protocol. Set `native_inference: true`, a model `endpoint_id`,
`network: deny`, and `budget.max_subagents: 0`. Explicitly allow `inference.use`
target `cerynth` and `credential.use` target `inference-session`, in addition to
workspace grants and any required read-only native-binary directory. Set owner
overrides `CERYNTH_GUARDD_BINARY` and `CERYNTH_GUARD_EXEC_BINARY` if the pinned
binaries are not beside agentd. The runtime starts at most two bounded gateways,
each holding only its root's revocable grant outside the namespace. Provider
credentials remain in the inference service. A regular-file Landlock grant can
produce a partial profile on this pinned Guard; grant only the intended dedicated
binary directory, or let Guard fail closed. Do not enable partial Landlock.
Native delegation and GPU grants are unsupported in this mode. See the
[integration design and limits](../architecture/guard-inference-handoff.md).

## Model and GPU control

The coordinator has distinct owner and token data sockets. Its optional HTTP
surface exposes only `/v1/models`, `/v1/chat/completions`, and `/v1/responses`.
It authenticates each request, bounds output, holds queue slots through stream
completion, and applies session/ancestor concurrency scopes. Direct HTTP calls to
the underlying model server bypass this cooperative admission policy.

Admin operations inspect installed/running models and details, unload models
only when idle, and start/stop explicitly owned Ollama backends. A backend launch
requires an absolute binary, model directory, loopback endpoint, context length,
and optional parallel-request count. The packaged inference service's 8 GiB
memory limit includes its owned backend descendants; adjust it deliberately for
larger models. External servers have their own resource limits.

`inferenced discover http://127.0.0.1:11434` probes only explicit numeric-loopback
origins, with a one-second per-candidate deadline and no automatic registration.
`inferenced registry local --model qwen2.5:3b` returns normalized model details,
including architecture versus active context limits, quantization, weight size
and observed residency. Unknown device/VRAM/license fields stay null. Admin
`cached_models` returns persisted observations labelled as cached; restart
invalidates residency until a new backend observation.

OpenCode discovery is separately configured with owner `CERYNTH_OPENCODE_URL`
and optional `CERYNTH_OPENCODE_PASSWORD_ENV` naming the environment variable that
contains its HTTP password. `agentd discover --harness open_code` lists native
session metadata. It never imports a native session, claims a sandbox or acquires
process control. Other harness discovery remains unsupported.

Compatible remote endpoints require owner consent, HTTPS, an environment-variable
name for credentials, and per-session cloud consent. There is no automatic cloud
fallback. Backend origin bindings cannot be reassigned to a different trust
domain across revocation/restart. Use genuinely isolated engines for distinct
trust domains; URL differences alone do not prove cache isolation.

A local compatible endpoint may explicitly select
`"cache_isolation":"vllm_per_grant_salt"`. For vLLM 0.25.0 native prefix caching,
the coordinator then injects a private salt derived from the authenticated grant
on both data planes. Agents cannot supply their own salt. Credential rotation
also rotates the cache scope. Responses transport is rejected in this mode;
only chat was validated. Origin/trust-domain restrictions remain in force.
This is an owner-declared, backend-specific capability, not automatic detection.
Do not select it for Ollama, SGLang, LMCache/external KV connectors or an unverified
server: their cache keys were not validated by this integration.

`cerynth-hardware` reports CPU, RAM, driver and GPU observations. GPU admission
reserves a desktop margin and tracks cooperative reservations. `gpu_run` launches
an admitted job only under an owned generic Guard session with `gpu.execute`, an
adequate cooperative budget and explicit `gpu.use: *`. Guard's existing GPU
profile exposes all supported device nodes, so it is not per-device partitioning.
Some CUDA jobs require an explicit read grant for `/sys`. Plain `start` cannot
bypass the admitted-job gate. `gpu_run` fixes job/operation IDs and the launch
fingerprint, records Guard's decision, and retains the lease until verified scope
cleanup. A supervised lease cannot expire or be manually freed while its process
remains. Person 2 labels this non-Git policy decision advisory; agentd separately
enforces the launch gate and preserves both facts in the receipt. Memory remains
a cooperative reservation. Preemption, checkpointing, hard VRAM limits and
MIG/MPS partitions are not implemented.

Owner inference operation `plan_placement` compares fresh device observations,
protected/active residents, calibrated allocation sizes and measured switching
samples. It returns advisory choices and cost ranges; stale/unknown or unauthorized
targets are queued/rejected rather than guessed. It never evicts or moves a model.
CacheWeaver also remains a recommendation policy; experimental LMCache transfers
were performed by the engine, not by a new Cerynth KV implementation.

## A2A and optional remote prototype

`agentd a2a-gateway --sponsor UUID --listen 127.0.0.1:PORT --credential-file /private/path/token`
starts a loopback A2A 0.3 JSON-RPC subset bound to a live sponsor with
`task.delegate`. The new 0600 credential file is removed on graceful shutdown.
Its parent must be inside the private agentd control directory, outside workspaces.
The Agent Card is at `/.well-known/agent-card.json`; authenticated requests use
`/a2a`. Only text `message/send`, `tasks/get` and `tasks/cancel` are supported.
Message IDs deduplicate immutable task details. Caller metadata cannot choose
capabilities, sponsor, credentials or budgets. Submitted tasks are drafts; the
owner starts execution separately. Streaming, push, file contents and full A2A
conformance are not advertised. A killed gateway leaves a stale credential file;
the owner must remove it before an explicit restart, which rotates the token.

`agentd remote CONFIG.json REQUEST.json` sends one versioned request through SSH
to the fixed `agentd relay` entry point. Configuration requires explicit remote
consent, a numeric host, dedicated private key, pinned known-hosts file, remote
binary and private state paths. Ambient SSH config/agent authentication,
forwarding and connection multiplexing are disabled. The relay calls the remote
owner socket; this is a powerful owner credential, not an agent capability.
Only an ephemeral-key loopback sshd was tested, not a physical second machine.
Transport failure can leave an unknown remote outcome; query the durable operation
ID before retrying. There is no network-partition rollback or remote kill guarantee.

## Packaging and verification

```bash
cargo build --locked --release -p cerynth-agentd -p cerynth-inferenced -p cerynth-resources
scripts/install-agent-runtime.sh --root /absolute/staging/root
```

The staging script copies binaries, user units and this guide. It enables no
services and installs nothing into the live root. Person 1 still needs to review
image integration. The units connect the supervisor to the local coordinator;
the owner must still configure endpoints and install models/harnesses.

Ordinary tests use synthetic backends and do not require accounts, models, a GPU,
or working systemd user scopes. Opt-in host checks:

```bash
python3 tools/agent_runtime_smoke.py
python3 tools/worktree_runtime_smoke.py
python3 tools/native_guard_runtime_smoke.py
python3 tools/gpu_runtime_smoke.py
python3 tools/task_evidence_runtime_smoke.py
python3 tools/a2a_runtime_smoke.py
python3 tools/remote_runtime_smoke.py
python3 tools/agent_runtime_stress.py --cycles 100
python3 tools/guard_runtime_smoke.py
python3 tools/inference_runtime_smoke.py --model qwen3:0.6b
python3 tools/harness_runtime_smoke.py --brokered --harness open_code --model qwen2.5:3b
python3 tools/shared_model_benchmark.py --models-directory /absolute/model/cache
python3 tools/cache_baseline.py --models-directory /absolute/model/cache
python3 tools/opencode_api_smoke.py --opencode /absolute/path/to/opencode
```

Host checks create their own temporary services/workspaces and clean them up.
Inference/harness smoke checks expect an already running local Ollama server;
the shared-model benchmark owns the servers it starts. Raw results go under
ignored `artifacts/person3`. Retain failed trials alongside successful ones.
The Guard test requires separately built binaries from the pinned Guard revision.
The coding checker imports generated Python only inside a fresh managed sandbox
with networking denied. Its two fixed cases are a synthetic functional check,
not proof of arbitrary code correctness. Older trials used a host-side checker
and are labelled legacy in the [validation report](../integration/2026-11-11-person3-runtime.md).
Sanitized numerical evidence is retained under `docs/benchmarks/person3`.

For startup failures, run `diagnostics`, check `systemctl --user status`, verify
Bubblewrap namespaces and executable paths, and confirm directory permissions.
Backend outages fail requests with explicit errors rather than selecting another
provider. Cancellation closes the client request; GPU work may continue inside
the engine. Revoke tokens before changing endpoint policy and stop sessions
before stopping a backend. Never interpret CLI exit zero as verified coding
success. Cross-track integration and outstanding experiments are listed in the
task ledger.
