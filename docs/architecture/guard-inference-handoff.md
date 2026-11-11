# Guard and inference identity handoff

Date: 2026-11-11. Status: proposed integration; not an approved shared contract.

The offline integration uses Guard revision
`e592fa47797179ad24a99fc76143be48191a7649`. It does not support native model
traffic yet. Guard's gateway strips incoming authorization, while `inferenced`
requires a per-session credential. Giving every gateway one shared grant would
erase session attribution and defeat per-session model/concurrency policy.

The required behavior is a trusted binding from a registered Guard session to
an immutable coordinator grant. Agents must never select another session ID,
upstream token, trust domain, model allowlist or remote provider. Administrative
and approval sockets must stay outside the agent namespace.

Proposed responsibilities:

| Component | Responsibility |
| --- | --- |
| agentd | Register the same session UUID with Guard and the coordinator; journal both leases before starting; bind exact model/endpoint/action details; revoke on failure/exit |
| Guard | Authenticate the namespace's fixed gateway, enforce permitted remote routes and method/path bounds, remove agent-supplied authorization and identity headers |
| Trusted gateway integration | Attach the matching short-lived upstream grant outside the namespace; reject mismatched identity, expiry and origin; never expose the owner control API |
| inferenced | Authenticate the grant, enforce model and session/parent/project/global limits, apply origin/trust-domain policy and record actual request identity |

Two implementation options remain to evaluate: an explicit per-session upstream
credential binding in Guard, or a narrowly scoped per-session trusted bridge
outside the namespace. A bridge would need bounded lifetime/resources, an
unforgeable gateway-to-bridge binding and failure cleanup. A plain unauthenticated
shared loopback proxy does not establish that binding. This document changes no
Guard wire fields and does not assume either option is accepted.

Before native launch is enabled, exercise these concrete acceptance cases:

1. Two Guard sessions use one local backend with distinct grants and observable
   session/parent/project limits; no administrative socket or host credential is visible.
2. Session A supplies B's identity, bearer token and model selection in headers/body;
   its request is denied or remains attributed strictly to A.
3. Denied domains, unapproved cloud destinations and non-inference routes remain
   denied even when the agent can run arbitrary shell code.
4. Revocation, deadline, coordinator restart, Guard restart and daemon death release
   the correct leases and stop streams without a weaker network fallback.
5. A permission denial produces a fixed audit reason without prompt/token leakage.
6. A real coding task and its independent checker run inside the intended boundaries;
   process exit, tool evidence and verified task results are reported separately.

Team review is needed to stabilize shared semantics. The missing implementation
and tests are outstanding engineering work, not completed by this proposal.
