> Updated gateway implementation and tests: see [egress.md](egress.md). Earlier limitations below record prior validation stages.

# Guard validation and outstanding security work

This is a prototype, not a release security review. The prior handoff contains
claims that need qualification. The active checkout is `~/Projects/CerynthOS`.

## Verified on the development host

- `cargo test -p cerynth-security -p cerynth-guardd`: 42 unit tests passed; the sandbox process integration test also passed explicitly.
- `GUARD_LAUNCHER=$PWD/target/debug/cerynth-guard-run tests/security/sandbox-boundary.sh`:
  35 boundary checks passed on this host, including a GPU render-node denial.
  A disposable copy of the Cerynth VM (kernel 7.1.0+) passed 34 checks;
  no GPU was attached to the VM. Neither run used a real agent harness.
- Parent approval restrictions cannot be downgraded in child capabilities;
  child registration requiring spawn approval is rejected until an approval
  path exists; a parent with active children cannot be ended.
- Approval claims are marked in progress before awaiting broker execution.
  An in-progress approval cannot be claimed again.
- Filesystem and GPU profiles with deny or approval mount rules fail closed:
  mounts cannot implement per-operation approvals or exclusions.

## Important limitations

- The default sandbox mounts the selected workspace read-write even if no
  filesystem capability is listed. It does not implement read-only workspaces,
  per-file approvals, or exclusion rules. Select only a workspace the agent may
  fully modify. Extra allow mounts grant direct access for the session lifetime.
- An experimental local socket server exists, but has not been integrated with
  the runtime or desktop. The control socket trusts the daemon Unix user; each
  session socket is a bearer capability. Sandboxes must see only their own
  socket, and the host user is trusted. The daemon and Git broker are not a
  complete reference monitor.
- The sandbox has no direct host networking. The fixed-endpoint inference
  gateway is implemented and fixture-tested; live provider authentication,
  cross-session adversarial review and arbitrary domain egress are unvalidated.
- `git.push` has not had an adversarial integration review; the workspace and
  remote are untrusted, and credential forwarding and redirect handling need
  careful verification before enabling this path in a service.
- Audit chains detect modification within the retained chain, but cannot prove
  a valid tail was not truncated without an independently stored checkpoint.
  Failure handling is incomplete: most decisions and session lifecycle events
  can currently proceed when audit writes fail. Git push now records intent
  before its side effect, but completion logging can still fail.
- Sessions are in memory. On restart, audit recovery marks unclaimed approvals
  cancelled and claimed approvals indeterminate; it does not restore sessions
  or retry actions. Revocation while an operation executes, bounded retention,
  and durable exactly-once execution remain unfinished.
- SecurityDecision's `enforced` value is not yet proof that every action is
  intercepted. Sandbox enforcement and broker decisions must be reported
  separately after integration.
- Other pending work includes real-harness IPC integration, independently
  reviewed caller authorization, audit durability, egress and credential lifecycle,
  supply-chain review, penetration and usability evaluation, P1/P3/P4 contract
  integration, VM and GPU checks, benchmark trials, packaging and release review.

The example systemd unit runs under a dedicated service account; its owner-only
socket is inaccessible to other user accounts. Integration requires a reviewed
authentication and workspace-access design before enabling the unit.

The Git broker passed a disposable local push test, confirming the exact commit
and that a workspace pre-push hook did not run. HTTPS redirect following is
disabled. This is not a comprehensive adversarial review. The audit log now
holds an exclusive advisory writer lock; verify its behavior on the deployment
filesystem.

## Runtime client and user packaging validation

- Added typed bounded client and `cerynth-guardctl` control CLI.
- Passed explicit `managed_execution_and_cleanup` process test: real daemon,
  session registration, real bwrap/Landlock/seccomp launch, workspace output,
  denied trusted operations, session socket cleanup and verified audit.
- Ran `scripts/guard/install-user.sh`, then started the installed user service,
  queried its status successfully, and stopped it. The service remains disabled
  at login. Installation preserved existing configuration.
- Added `runtime-integration.md` with the P1/P3/P4 integration sequence and
  concrete distinctions between process cleanup and authorization cleanup.
- Clippy for the added client, CLI and integration test passed; the documented
  unsafe flock warning and upstream unused manifest key warning remain.

These client and packaging checks do not validate an actual harness runtime,
Qt desktop, or complete the public-alpha security review. Gateway evidence follows.


## Inference egress implementation

Implemented a per-session fixed-endpoint gateway using Axum and reqwest, attached
to the session socket lifecycle. Added exact endpoint grants, credential grants,
vault header injection with origin binding, redirect blocking, request size and
time bounds, request header filtering, inference-only routes, and streaming.
The process integration test now launches a real sandbox and reaches a loopback
model fixture through the gateway. Unit tests validate injected vault credentials,
stripped client credentials, unknown endpoints, redirects and ended sessions.
These are fixture tests, not a real cloud-provider or full harness validation.

Use scripts/guard/validate.sh to rerun local checks and collect evidence.
