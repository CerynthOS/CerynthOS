# Adapter execution belongs to the supervisor

Date: 2026-11-11. Status: implemented experimental decision; cross-track review pending.

The runtime must apply one identity, operation ledger, resource scope and execution
boundary regardless of harness. Allowing each adapter to spawn independently
would duplicate cleanup, credential handling and authorization, and could bypass
the supervisor's journal.

`cerynth-agent-api::HarnessAdapter` therefore exposes a descriptor, installation
check and `prepare_launch(session, input)`. `AdapterInput` distinguishes a native
prompt from an explicit generic command. The result is a `LaunchRequest`, with
no process started. Native prompts use stdin and are absent from argv. The
Generic PTY and four native CLI drivers use this interface. Identity mismatches,
unsupported input kinds and oversized prompts fail before execution.

`cerynth-agentd` validates ownership/dependencies/budgets, journals the immutable
operation, constructs the resource and sandbox boundary, adds a scoped inference
grant when configured, starts the process, streams bounded output, and performs
cleanup. A prepared launch grants no capability by itself. The generic PTY crate
provides terminal transport only after the supervisor has constructed this plan.

Descriptors advertise only implemented behavior. Native CLI adapters expose
launch, structured events and supervised cancellation. Native pause/resume,
credential transfer, tool approval and native-subagent control are absent. Generic
PTY supports basic input/output and cancellation without semantic tool events.
OpenCode API discovery is a separate configured capability and does not acquire
control over discovered processes. ACP's negotiated protocol client remains a
separate transport until a daemon execution recipe is implemented and validated.

Contract tests cover each CLI driver, stdin prompt privacy, capability omissions,
kind/input mismatches and opaque backend model names. Live host tests separately
establish actual boundaries and record task failures. A version probe or launch
plan cannot establish live provider authentication or correct agent output.
