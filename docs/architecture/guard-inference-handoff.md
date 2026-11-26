# Guard and inference identity integration

Date: 2026-11-11. Status: implemented opt-in development slice; shared semantics
remain subject to team review. The weekly plan is unchanged.

The runtime uses Person 2's existing Guard configuration and wire protocol at
`e592fa47797179ad24a99fc76143be48191a7649`, without modifying or merging it.
Each native root gets a dedicated Guard gateway holding only that root's
short-lived coordinator grant. The upstream provider credential stays in
`inferenced`. Neither credential is passed to the agent.

```mermaid
flowchart LR
    A[OpenCode namespace] -->|fixed local forwarder| G[Owned per-root Guard gateway]
    G -->|root-specific narrow grant| I[Inference coordinator]
    I -->|service-only credential if configured| M[Authorized model backend]
```

The gateway removes incoming authorization and identity headers and injects its
own immutable grant. The coordinator authenticates that grant, limits models,
queues requests under session/project budgets, and records actual session identity.
There is no administrative HTTP route. The agent namespace cannot reach host
loopback endpoints directly or mount the control/gateway directories.

`runtime.guard.native_inference: true` requires an OpenCode root session, a named
coordinator endpoint, `max_subagents: 0`, and explicit allow grants for
`inference.use: cerynth` and `credential.use: inference-session`. Native child
delegation, arbitrary endpoint selection, provider credential exposure and GPU
device grants are unsupported in this mode. Offline generic Guard sessions retain
their existing inheritance integration.

There are at most two private native gateways. Each has its own systemd scope,
96/128 MiB memory high/max thresholds, CPUWeight 50, TasksMax 128 and kernel
parent-death protection. Scope and authority preparation are journalled before
launch; the final cancellation check and physical process ownership transfer
share the runtime lock. A cancellation during provisioning cannot later launch
the agent. Terminal cleanup ends the Guard registration and stops the exact
owned gateway scope. Startup recovery empties both scopes and revokes the old
grant before serving clients.

The provisioning file is owner-only and removed once Guard loads its vault.
Interrupted provisioning is scrubbed during recovery. It contains a revocable
session grant, never a provider key. Private audit/configuration files remain
for inspection; operators must manage retained evidence storage.

`tools/native_guard_runtime_smoke.py` uses the actual pinned Guard binaries and
an adversarial synthetic harness. It verifies two roots, forged identity headers,
model denial, host network/files/control denial, credential invisibility,
startup cancellation, SIGKILL recovery and grant/scope cleanup. Actual OpenCode
and Ollama trials also ran inside this boundary; failed coding results are
retained separately from successful transport and security checks.

This is not authoritative deduplication of arbitrary external harness tools.
Native provider/endpoint variants, delegated native gateways and production
security review remain outstanding. No teammate approval is inferred from the
working local integration.
