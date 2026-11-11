# Harness and protocol research

Research date: November 10, 2026. Upstream capabilities and Cerynth implementation
status are separate columns. Provider authentication is never assumed transferable.

## Harness compatibility matrix (P3-002, P3-004, P3-006)

| Harness | Official integration surfaces | Lifecycle and output | Tools / subagents | Cerynth status |
| --- | --- | --- | --- | --- |
| OpenCode | Headless HTTP server/OpenAPI, CLI JSON, ACP | Sessions, prompts, message history, event stream, abort | Native tool events and agent facilities; native child sessions need explicit mapping | CLI launch plan plus bounded HTTP client; real CLI tried at 1.19.5; coding success is verified separately |
| Hermes | CLI, ACP stdio, native Python interfaces | One-shot query/file input, JSONL where supported, native resume | Harness-native delegation/tooling; not equivalent to OS child budgets | Pinned 0.23.1 launches inside Bubblewrap/systemd and uses authenticated local inference; observed tool failures prevent claiming verified coding success |
| Codex | Exec JSONL, app-server protocol, SDK | Turns/threads, local history, interrupt/resume depend on interface | Structured execution/results/usage; native collaboration is not automatic Cerynth delegation | Installed 0.167.0 exercised with local Ollama; explicit provider override and daemon isolation required |
| Claude Code | Programmatic CLI/Agent SDK | Print mode, streaming JSON, session resume; CLI exit status alone does not verify task success | SDK permissions and native subagents; trusted OS enforcement remains separate | CLI launch plan and version discovery; installed 2.1.312; cloud execution/credential broker not exercised |
| Generic CLI/PTY | Process argv, pipes, PTY | Start, input, output, process-group stop, deadline | Structured tools/resume/subagents are unavailable unless an adapter reports them | Implemented and tested with actual CLI and PTY processes |

Sources: [OpenCode server](https://opencode.ai/v1/docs/server/),
[Hermes CLI](https://hermes-agent.nousresearch.com/docs/reference/cli-commands),
[Codex app-server](https://learn.chatgpt.com/docs/app-server),
[Codex non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode),
[Claude Code programmatic CLI](https://code.claude.com/docs/en/headless),
[Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference).

Initial local inference backend: Ollama. It is already installed, provides model
inventory/details/loading observations, supports CPU/GPU operation, and exposes
native streaming plus a compatible API. OpenCode is the reference coding harness;
Codex is the already-installed second local integration experiment. Initial
selection does not imply that a tiny local model reliably completes coding tasks.

The pinned OpenCode 1.19.5 binary was downloaded to ignored test artifacts and
verified against the release API's SHA-256 digest. Current OpenCode documentation
also covers a redesigned configuration; consult the pinned release source rather
than assuming current examples apply unchanged to a 1.x binary.

## Protocol roles (P3-003)

| Interface | Purpose | Implementation boundary |
| --- | --- | --- |
| ACP v1 | Client-to-agent sessions, prompts, updates, permissions | Bounded client negotiates version/capabilities; no FS/terminal capabilities advertised; ungranted permission requests are denied |
| MCP 2025-11-25 | Tools, resources, prompts, negotiated protocol | Initialization envelope is pinned; the authoritative tool broker is not implemented by this track |
| A2A 0.3.0 | Inter-agent task/message interoperability | Experimental owner-API subset for message/send and task get/cancel; no public HTTP server or full conformance claim |
| OpenAI-compatible HTTP | Model inference transport | Configured endpoint, allowlisted model, explicit remote consent, TLS for remote routing; API subsets vary by backend |
| Cerynth Unix API | Trusted owner control, sessions, resources, diagnostics | Separate version from frozen scheduler IPC; agent sandboxes must not see this socket |

Sources: [ACP initialization](https://agentclientprotocol.com/protocol/v1/initialization),
[ACP sessions](https://agentclientprotocol.com/protocol/v1/session-setup),
[ACP prompt updates](https://agentclientprotocol.com/protocol/v1/prompt-turn),
[MCP specification](https://modelcontextprotocol.io/specification/2025-11-25),
[A2A specification](https://a2a-protocol.org/latest/specification/),
[Ollama compatibility](https://docs.ollama.com/api/openai-compatibility).

The latest A2A document is research context, not evidence that the internal 0.3
subset implements the latest protocol. No protocol name itself grants resource,
filesystem, credential, network, or delegation authority. A2A task text and
harness notifications remain lower-trust data.

## Hardware inventory and accelerator limits (P3-005, P3-061, P3-074)

Measured on the development host:

| Component | Observation | Enforced capability |
| --- | --- | --- |
| CPU | Intel i7-13700HX, 24 logical CPUs | systemd CPUWeight; custom scheduler weight semantics need Person 1 validation |
| RAM | 16,449,400,832 bytes | Per-scope MemoryHigh/MemoryMax verified |
| NVIDIA | RTX 5060 Laptop GPU, 8,151 MiB, driver 615.78.08 | Utilization, total/used memory and supported compute-process memory; cooperative reservations only |
| Intel | Integrated GPU using i915 | DRM discovery; dedicated VRAM metrics unavailable on this host |
| MIG | nvidia-smi reports N/A | No enforced partition advertised |
| MPS | Not configured or verified | No memory or SM guarantee advertised |
| Local inference | Ollama 0.35.0, CUDA compute 12.0 reported by backend | Engine-owned model placement and allocation; Cerynth does not override driver limits |

Hardware discovery is executable with `cerynth-hardware`, retains unavailable
fields as null, and bounds external telemetry commands. NVIDIA compute-process
metrics do not capture every desktop graphics allocation. Observed free memory
and cooperative reservations can overlap; the broker deliberately accounts
conservatively and reserves 1 GiB for desktop/safety by default.

Sources: [NVIDIA supported MIG GPUs](https://docs.nvidia.com/datacenter/tesla/mig-user-guide/supported-gpus.html),
[NVIDIA MPS documentation](https://docs.nvidia.com/deploy/mps/).

## Cache and backend research boundaries

`cerynth-model-api::cache` implements an experimental deterministic policy over
explicit metadata. It protects active/interactive entries, respects trust
domains, and compares observed reload/recompute costs against supported transfer
costs. Offloading is recommended only when a caller advertises that capability.
Ollama does not acquire a new KV offload API merely because this policy exists.

The host's default Python is 3.14.8 and torch, vLLM, SGLang, and LMCache are not
installed. No vLLM/SGLang/LMCache throughput or offload result is claimed.
Separate environments and backend-specific capability tests are required before
choosing those engines. Current measurements cover real Ollama endpoint behavior
and report failed coding workflows alongside successful inference requests.

Sources for subsequent engine evaluation:
[vLLM](https://docs.vllm.ai/), [SGLang](https://docs.sglang.ai/),
[LMCache](https://docs.lmcache.ai/).
