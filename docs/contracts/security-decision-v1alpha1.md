# Contract: SecurityDecision `v1alpha1`

Status: **proposal, needs sign-off from P1, P3 and P4 in Sprint 1.**
Owner: Person 2 (Cerynth Guard).

This contract defines how any Cerynth component asks Guard "may this session
do X?" and how Guard answers. It covers capabilities, action requests,
decisions, approvals and the events Guard emits. It does not cover
resource budgets (P1's `ResourceBudget`) or session identity (P3's
`AgentSession`); it refers to them by ID.

## Rules every consumer can rely on

1. **Deny by default.** Anything not explicitly allowed is denied.
2. **Sessions are identified by `session_id` (UUID), never by PID.** Guard maps PIDs to sessions using the cgroup/scope that P1 creates.
3. **Every answer says whether it is enforced.** `enforcement: "enforced"` means the OS boundary makes the other outcome impossible. `"advisory"` means Guard is relying on the harness to honour it (Integrated mode). Never show advisory as enforced.
4. **Approvals are bound to one exact operation.** Changing any argument after approval invalidates it.
5. **Guard executes brokered actions itself** (for example `git push`), using credentials the agent never sees. The agent receives the result, not the token.
6. **Child sessions get a subset of their parent's capabilities.** A request for more is denied with `reason: capability_exceeds_parent`.
7. **Schema versions are explicit.** Unknown fields are ignored; unknown `kind` values are denied.

## Capabilities

A capability is a typed grant attached to a session.

```json
{ "kind": "fs.write",    "target": "workspace" }
{ "kind": "fs.read",     "target": "/usr/share/dict" }
{ "kind": "net.egress",  "target": "api.anthropic.com:443" }
{ "kind": "git.push",    "target": "origin:refs/heads/agent/*", "mode": "approval" }
{ "kind": "credential.use", "target": "anthropic-default" }
{ "kind": "session.spawn", "target": "child", "limit": 2 }
{ "kind": "tool.exec",   "target": "cargo" }
```

`mode` is `"allow"` (default), `"approval"` or `"deny"`. `target` patterns use
`*` only as a trailing wildcard. `workspace` is a symbolic target that
resolves to the session's workspace path.

## ActionRequest (caller → Guard)

```json
{
  "schema": "cerynth.guard/v1alpha1",
  "request_id": "6f1c0c2e-6f53-4c1d-9b55-6a0d3a1e9a10",
  "session_id": "0d8f6b8a-2c1b-4a5e-8a0e-2f9a5c6d1e44",
  "action": {
    "kind": "git.push",
    "target": "origin:refs/heads/agent/fix-login",
    "args": { "force": false, "commits": ["a1b2c3d"] }
  },
  "context": {
    "harness": "opencode",
    "origin": "agent",
    "provenance": ["file:/workspace/README.md"]
  }
}
```

`context.origin` is one of `agent`, `user`, `system`. `provenance` is optional
in v1alpha1; it records where the instruction came from, for the R4 research
track.

## SecurityDecision (Guard → caller)

```json
{
  "schema": "cerynth.guard/v1alpha1",
  "request_id": "6f1c0c2e-6f53-4c1d-9b55-6a0d3a1e9a10",
  "session_id": "0d8f6b8a-2c1b-4a5e-8a0e-2f9a5c6d1e44",
  "decision": "approval_required",
  "reason": "policy_requires_approval",
  "policy_id": "default/git-push-requires-approval",
  "enforcement": "enforced",
  "approval": {
    "approval_id": "b7f0d1d2-5f2e-4b8e-9c61-1b2a3c4d5e6f",
    "digest": "sha256:3f5a…",
    "summary": "Push 1 commit (a1b2c3d) to origin agent/fix-login",
    "risk": "medium",
    "expires_at": "2026-11-12T10:15:00Z"
  },
  "decided_at": "2026-11-12T10:05:00Z"
}
```

`decision`: `allow` | `deny` | `approval_required`.

`reason` (stable codes, for UI text and tests):
`policy_allow`, `policy_deny`, `policy_requires_approval`, `no_matching_capability`,
`capability_exceeds_parent`, `approval_expired`, `approval_mismatch`,
`session_unknown`, `session_not_managed`, `unknown_action_kind`, `guard_unavailable`.

If Guard is unavailable, callers **must** treat the result as `deny` with
`reason: guard_unavailable`. They must not fall back to allow.

## Approval flow

```
agent ──ActionRequest──▶ guardd ──decision: approval_required──▶ agentd (holds action)
                           │
                           └── event guard.approval.requested ──▶ desktop Security Center (P4)
user approves in desktop ──ApprovalResponse{approval_id, digest, approve}──▶ guardd
guardd re-checks digest + expiry, executes the action itself, emits guard.approval.resolved
```

```json
{ "schema": "cerynth.guard/v1alpha1", "approval_id": "b7f0…", "digest": "sha256:3f5a…",
  "response": "approve", "responder": "uid:1000", "scope": "once" }
```

`scope` is `once` in v1alpha1. Remembered approvals ("always allow this") are a
v1beta1 topic, because they turn into policy and need their own review.

The digest is SHA-256 over the canonical JSON (sorted keys, no whitespace) of
`{session_id, action}`. The desktop shows `summary`, but approves `digest`.

## Events (`CerynthEvent` payloads, envelope owned by P4)

| `type` | When | Key fields |
|---|---|---|
| `guard.decision` | Every decision | `request_id`, `session_id`, `decision`, `reason`, `action.kind` |
| `guard.approval.requested` | Approval needed | `approval_id`, `summary`, `risk`, `expires_at` |
| `guard.approval.resolved` | Approved, denied or expired | `approval_id`, `response`, `executed` |
| `guard.violation` | The sandbox blocked something outside a request (for example a seccomp kill) | `session_id`, `mechanism`, `detail` |
| `guard.session.confined` | Sandbox created | `session_id`, `mode`, `profile`, `layers` |

Events never contain secrets, raw prompts or file contents. `args` are
included only for action kinds marked safe to log.

## Transport

- `cerynth-guardd` listens on `/run/cerynth/guard.sock`, owned `root:cerynth-guard 0660`.
- Callers are identified with `SO_PEERCRED`. Only agentd, cerynthd and the desktop's approval helper are allowed to call it.
- Sandboxed agents **never** get this socket. They reach brokered tools through a per-session socket that guardd creates inside the sandbox, which only accepts `ActionRequest`s for that session's own `session_id`.
- Framing: newline-delimited JSON, the same as cerynthd's IPC, so the existing `cerynth-ipc` patterns can be reused.

## Open questions for sign-off

- **P1:** can the resource scope expose a stable `session_id ↔ cgroup` mapping that guardd can query?
- **P3:** which harnesses can route tool calls through a broker (MCP, ACP, hooks), and which need the generic PTY path where Guard can only confine, not mediate?
- **P4:** is the event envelope's `session_id` and `trace_id` enough to group Guard events into the session timeline?
- **All:** are the `reason` codes enough for the UI to show useful, specific messages?
