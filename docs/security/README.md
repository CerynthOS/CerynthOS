# Cerynth Guard: Person 2 plan

**Owner:** Person 2 (Agent Security & Trusted Execution)
**Status:** Sprint 1 (Nov 9–15, 2026), prototype boundary working
**Related:** [threat model](threat-model.md) · [SecurityDecision contract](../contracts/security-decision-v1alpha1.md)

## 1. The job

> Agents do legitimate work **without** being able to reach the host, the
> user's secrets, the Cerynth control plane or other agents beyond what their
> session was granted, even when the agent itself is compromised.

Guard is CerynthOS's reference monitor. It decides what a session may do, and
it makes the OS enforce that decision. It is **not** a prompt-injection
classifier. It assumes the agent may be hostile and limits the damage
regardless ([threat model §1](threat-model.md#1-the-core-assumption)).

### Ownership boundaries (from the team plan)

| Owner | Owns | Guard's relationship |
|---|---|---|
| **P2 Guard** | Whether an operation is **authorized**, and the sandbox boundary that enforces it | — |
| P1 Platform | The process and its **resource** boundary (cgroups, scopes, scheduler) | Guard places the sandbox *inside* P1's scope; P1 limits pids, memory and disk so a sandboxed agent can't exhaust the machine |
| P3 Runtime | The logical **session** (identity, harness, lifecycle, subagents) | agentd asks Guard for a confinement profile before launching and sends tool calls to the broker |
| P4 Desktop | **Presentation** and observability | Security Center shows Guard's decisions, approvals and audit; it never decides by itself |

Resource *adaptation* policy (`crates/cerynth-policy`, which profile to run)
and *authorization* policy (Guard) are separate systems, even though both are
called "policy". Guard's crate is `cerynth-security`, and its policies live in
`/etc/cerynth/guard/`.

## 2. What Guard is made of

| # | Component | What it does | Build or reuse |
|---|---|---|---|
| A | **Threat model** | What we protect, from whom, what we don't promise | Write (✅ v0) |
| B | **Sandbox launcher** | Starts a session's process tree inside an isolation profile | Reuse **bubblewrap + Landlock + seccomp**; we build the profile generation |
| C | **Policy engine** | Turns policy and capabilities into allow / deny / approval | Reuse **Cedar** (decision pending, Sprint 1 prototype); we build the capability model |
| D | **Action broker** | Executes risky actions (git push, API calls, host writes) for the agent | Build, in `cerynth-guardd` |
| E | **Credential broker** | Holds secrets outside the sandbox and injects them per request | Build; reuse the system keyring (Secret Service) for storage |
| F | **Approval service** | Single-use approvals bound to one exact operation | Build; UI by P4 |
| G | **Audit log** | Hash-chained record of every decision | Build |
| H | **Security test suite** | Boundary, bypass and adversarial-workflow tests with controls | Build (✅ started) |

### Enforcement layers (defence in depth)

Each layer must stop the attack on its own. Later layers catch what earlier
ones miss.

| Layer | Mechanism | Stops | Status |
|---|---|---|---|
| L1 | **Mount + user + PID + IPC + UTS namespaces** (bubblewrap) | Seeing host files, processes, sockets | ✅ prototype |
| L2 | **Cleared environment with an allowlist** | API keys leaking through env vars | ✅ prototype |
| L3 | **Network namespace** (no network by default) | Exfiltration, unapproved APIs | ✅ deny-all; proxy in Phase 2 |
| L4 | **Landlock** filesystem and TCP rules, set by the launcher | A mount-namespace mistake still can't open host files | Sprint 2 |
| L5 | **seccomp** filter | Kernel attack surface (`bpf`, `keyctl`, `mount`, `io_uring` …) | Phase 2 |
| L6 | **Egress proxy** with a per-session allowlist and credential injection | Talking to anything except approved endpoints; the agent never holds the real API key | Phase 2 |
| L7 | **Action broker** for side effects | Destructive external actions without policy or approval | Phase 1–2 |
| L8 | Optional **strong profile**: rootless Podman or gVisor (`runsc`) | Kernel bugs reachable through namespaces | Phase 5 evaluation |

### Why these choices

- **bubblewrap first, not Podman.** bwrap is unprivileged, starts in milliseconds, confines one process tree, and is what Flatpak uses in production. It fits "one sandbox per agent session" on a desktop. Podman's container model (images, layers) is heavier than an agent workspace needs. It stays the candidate for the strong profile, alongside gVisor.
- **Landlock as a second filesystem layer.** It is unprivileged and stackable, and the Cerynth kernel and the dev host both support it (the host reports ABI 10). Landlock rules are set by the process on itself, so they survive even if a bind mount is configured wrongly.
- **Cedar over OPA (to be confirmed in P2-06).** Cedar has a native Rust crate, so it runs in-process in a Rust daemon with no extra service. Its policies can be analysed formally, and its *principal / action / resource* model maps directly onto *session / action / target*. OPA/Rego is more general but adds a Go process and a second language. The Sprint 1 prototype writes the same 5 policies in both and compares them.
- **Broker instead of trusting the harness.** Harness permission prompts (for example "allow this command?") run inside the agent's own process, so a compromised agent can bypass them. The broker sits outside the sandbox and holds the credentials, so bypassing it gets the agent nothing.

## 3. Architecture

```
            ┌────────────────── Cerynth Desktop (P4) ──────────────────┐
            │  Security Center: sessions · approvals · denials · audit │
            └──────────────────────────▲───────────────────────────────┘
                                       │ approvals (authenticated helper)
   cerynth-agentd (P3) ──profile?──▶ cerynth-guardd (P2) ◀── policy (/etc/cerynth/guard)
          │                            │   ├─ policy engine (Cedar)
          │ launch                     │   ├─ action broker ── git, APIs, host writes
          ▼                            │   ├─ credential broker ── keyring
   ┌─ P1 resource scope (cgroup) ──────┼─┐ └─ audit log (hash-chained)
   │  ┌─ Guard sandbox ──────────────┐ │ │
   │  │ harness + tools + shells     │◀┼─┘ per-session broker socket (only its own session_id)
   │  │ sees: /workspace (rw),       │ │
   │  │ system dirs (ro), private /tmp│ │
   │  │ no /run/cerynth, no ~, no net │ │
   │  └──────────────────────────────┘ │
   └────────────────────────────────────┘
```

## 4. What exists today (Sprint 1)

| Item | Path | Status |
|---|---|---|
| Sandbox prototype | `scripts/guard/guard-sandbox.sh` | ✅ working, unprivileged |
| Boundary test suite | `tests/security/sandbox-boundary.sh` | ✅ 25/25 pass, 6 controls confirm the attacks are real |
| Threat model v0 | `docs/security/threat-model.md` | ✅ draft for review |
| SecurityDecision contract | `docs/contracts/security-decision-v1alpha1.md` | ✅ proposal, needs sign-off |

Try it yourself:

```bash
tests/security/sandbox-boundary.sh
scripts/guard/guard-sandbox.sh --workspace ~/some-repo -- bash   # interactive sandboxed shell
```

What the suite proves today: inside the sandbox an agent can read and edit its
workspace, run tests, use git and spawn children. It **cannot**:
- read planted SSH keys or tokens, or your real `~/.ssh`
- escape with `../`
- write to `/usr` or `/etc`
- open network connections or resolve DNS
- see `/run/cerynth` or `/var/lib/cerynth`
- see host processes, use sudo or hold capabilities
- inherit API keys from the launcher's environment

## 5. Repository areas Person 2 owns

| Path | Purpose | When |
|---|---|---|
| `docs/security/` | Threat model, plan, decision records, security reports | now |
| `docs/contracts/security-decision-*.md` | The contract | now |
| `scripts/guard/` | Prototype launcher (replaced by guardd later) | now |
| `tests/security/` | Boundary, bypass and adversarial suites | now |
| `crates/cerynth-security/` | Types: capabilities, requests, decisions, approval digests; profile builder; Landlock/seccomp | Sprint 2 |
| `services/cerynth-guardd/` | The daemon: policy engine, broker, approvals, audit | Phase 1 |
| `crates/cerynth-policy-auth/` | Cedar integration and policy schema (or folded into cerynth-security) | Phase 1 |
| `packaging/guard/` | Default policies, systemd unit, `cerynth-guard` group | Phase 4 |

Create a directory only when real code needs it.

## 6. Roadmap

Each phase ends with an exit gate. A phase is not finished until its gate
passes, whatever the calendar says.

### Phase 0 — Baseline & contracts · W1–2 · Nov 9–22

| Week | Deliverable | Acceptance |
|---|---|---|
| W1 | Threat model v0, sandbox prototype, boundary tests, contract proposal, Cedar-vs-OPA prototype, Landlock prototype | See the Sprint 1 checklist (§7) |
| W2 | **Restricted workspace and network** as a real profile: `cerynth-security` crate with typed `Capability`/`ActionRequest`/`SecurityDecision` + a profile builder that generates the bwrap arguments from a session's capabilities; Landlock layer on by default; runs inside P1's scope | The same boundary suite passes when the profile comes from typed capabilities instead of hand-written flags; it also passes **in the Cerynth VM kernel** |

**Exit gate:** one managed agent process (P3's first launch) runs inside the Guard profile, and the boundary suite passes against it.

### Phase 1 — First managed agent · W3–4 · Nov 23–Dec 6

| Week | Deliverable | Acceptance |
|---|---|---|
| W3 | **Allow/deny broker**: `cerynth-guardd` skeleton on `/run/cerynth/guard.sock`, `SO_PEERCRED` caller auth, Cedar evaluation of `ActionRequest`s, deny-by-default, `guard.decision` events | Contract tests: unknown session → deny; unknown kind → deny; guard down → caller denies |
| W4 | **Approval prototype + audit**: approval digests, single-use, expiry, desktop approval helper endpoint (with P4), append-only hash-chained audit log | Approve A then send B → rejected (`approval_mismatch`); expired → rejected; the audit chain verifies; a broken chain is detected |

**Exit gate (Dec 6 team milestone):** a real coding agent finishes a task in its sandbox, and a forbidden file read is denied *without* breaking the task. A `git push` waits for approval in the desktop.

### Phase 2 — Security & multi-agent execution · W5–8 · Dec 7–Jan 3

| Week | Deliverable | Acceptance |
|---|---|---|
| W5 | **Tool enforcement**: broker-executed actions (`git.push`, `fs.write` outside the workspace, `net.egress`), and a Guard MCP server so harnesses call tools through the broker | A harness with no credentials pushes through the broker; a direct `git push` from inside the sandbox fails (no token, no network) |
| W6 | **Credential isolation**: keyring-backed credential broker; **egress proxy** that adds provider API keys outside the sandbox (the harness talks to `http://proxy` with a dummy key); redaction in logs and events | Canary key never appears inside the sandbox, in events or in the audit log; cloud agent works through the proxy |
| W7 | **Delegated capabilities**: child session gets a subset of its parent's capabilities, enforced by guardd; seccomp filter (L5) | Child asking for more → `capability_exceeds_parent`; blocked syscalls kill the process and emit `guard.violation` |
| W8 | **Adversarial regression suite v1**: ~50 scripted attacks plus legitimate workflows, run in CI where possible | Attack success 0 on enforced controls; legitimate task success reported alongside |

**Exit gate:** multiple harnesses in separate sandboxes, constrained subagents, no credentials in any sandbox, and an audit trail of everything.

### Phase 3 — Resource & inference coordination · W9–12 · Jan 4–31

| Week | Deliverable | Acceptance |
|---|---|---|
| W9 | **Broker-bypass testing**: try every path around the broker (other binaries, `/proc`, shared memory, sockets, ptrace, env, symlinks, hardlinks into the workspace) | Each bypass attempt is documented with its result; any success becomes a fix and a regression test |
| W10 | **Inference and tool authorization**: `inference.use` and `gpu.use` capabilities, so a session can only reach model endpoints it was granted (with P3) | A session without the grant can't reach the local model server |
| W11 | **Constrained GPU job security**: confine CUDA/Python GPU jobs (device nodes exposed per session only) | A GPU job runs; a session without `gpu.use` can't open `/dev/nvidia*` or `/dev/dri/*` |
| W12 | **Security benchmark evaluation (R4 v1)**: unrestricted vs harness-only permissions vs Guard | Report with attack success, legitimate task success, false blocks and overhead |

**Exit gate:** published R4 v1 numbers, including the cases where Guard blocked legitimate work.

### Phase 4 — Desktop developer alpha · W13–16 · Feb 1–28

| Week | Deliverable | Acceptance |
|---|---|---|
| W13 | **Desktop approval API hardening**: authenticated approval helper, polkit for admin-level policy changes | Only the logged-in user's desktop can approve; an agent can't approve its own request |
| W14 | **Audit persistence and error paths**: survives guardd restart, disk full, corrupt policy (fails closed) | Restart keeps the chain intact; corrupt policy means deny-all plus a clear error, never allow-all |
| W15 | **Penetration test week**: the whole team (and anyone willing) attacks the alpha | Findings tracked as issues; criticals fixed before W16 |
| W16 | **Security release review**: threat model v1, known limitations, default policies | Signed off; limitations listed in the alpha docs |

**Exit gate (Feb 28 developer alpha):** the security items of the alpha checklist: sandboxed execution with enforced filesystem limits, trusted tool authorization with human approval, honest audit evidence.

### Phases 5–7 — Mar–Sep 2027

| Phase | Weeks | Person 2 focus | Deliverable |
|---|---|---|---|
| 5 · Advanced optimization | W17–26 | Strong profile (gVisor/Podman) evaluation; policy usability (templates, "explain this denial"); credential lifecycles (rotation, revocation, scoped tokens); bypass tests | Hardened Guard security suite |
| 6 · Orchestration & research | W27–34 | Cross-agent trust: A2A/remote agents, delegated permissions across harnesses, remote-credential rules; **provenance / taint research** (does an action trace back to untrusted content?) | Secure delegation policy + R4/R5 report |
| 7 · Distribution & public alpha | W35–44 | Release threat model, secure defaults, supply-chain review (signed packages, dependency audit, SBOM), hardening of the image with P1 | Security release report |

## 7. Sprint 1 checklist (Nov 9–15)

| ID | Task | Status |
|---|---|---|
| P2-01 | Threat model v0: assets, adversaries, trust boundaries, threats → controls, non-goals | ✅ draft |
| P2-02 | Sandbox prototype: workspace rw, system ro, no home, no network, no control plane, cleared env | ✅ |
| P2-03 | Boundary test suite with controls (legitimate tasks must pass too) | ✅ 25/25 + 6 controls |
| P2-04 | `SecurityDecision v1alpha1` contract proposal | ✅ draft, needs sign-off |
| P2-05 | Landlock layer prototype (filesystem rules set inside the sandbox) | ⏳ |
| P2-06 | Cedar vs OPA: same 5 policies in both, decision record in `docs/security/decisions/` | ⏳ |
| P2-07 | Run the boundary suite in the Cerynth VM (kernel 7.1.0+) and record a support matrix | ⏳ |

**Sunday hand-in** (reviewer: Person 1): this document, the threat model, the
sandbox, the test results and the contract proposal. For the integration demo,
run P3's sample agent task inside `guard-sandbox.sh` and show the forbidden
file read being denied while the task still succeeds.

## 8. What I need from teammates, and what they get from me

| From | I need | By |
|---|---|---|
| P1 | A `session_id ↔ cgroup/scope` mapping; pids/memory/tmpfs limits for sandboxed trees; root-only control sockets; the VM kept current | W2 |
| P3 | The list of harnesses and what each needs (network endpoints, config dirs, tool hooks: MCP/ACP/hooks); agentd calls Guard **before** launch | W2 |
| P4 | The `CerynthEvent` envelope; Security Center screens for approvals and denials; approval helper UI | W3–4 |

| To | They get | By |
|---|---|---|
| P3 | `guard-sandbox.sh` today, then `cerynth-security::Profile` → launcher arguments | W1 / W2 |
| P4 | Mock `guard.*` event fixtures from the contract, so the UI can be built before guardd exists | W1 |
| P1 | The threat model for the control plane (asset A4) and requests for socket permissions | W1 |

## 9. Research track R4: trusted agent execution

**Question:** can unauthorized actions be stopped without making agents useless?

- **Workloads:** ~100 legitimate tasks (bug fixes, refactors, test runs, dependency updates) and ~100 attacks (secret theft, exfiltration, destructive git, persistence, subagent escalation, approval substitution), driven by injected content in files, issues, web pages and tool output.
- **Configurations:** unrestricted agent · harness permission prompts only · container only · Guard sandbox · Guard sandbox + broker + approvals.
- **Metrics:** attack success rate, legitimate task success rate, false-block rate, approvals per task (user burden), latency and CPU overhead.
- **Rule:** always report task success next to attack success. A defence that "wins" because the agent couldn't finish anything is a failure.

## 10. Reading list

| Topic | Read |
|---|---|
| Landlock | Kernel docs: *Landlock: unprivileged access control* (userspace API); the `landlock` Rust crate |
| Namespaces / bwrap | `bwrap(1)`, `user_namespaces(7)`, `mount_namespaces(7)`; how Flatpak builds its sandbox |
| seccomp | `seccomp(2)`; the `seccompiler` or `libseccomp` Rust bindings |
| Policy | Cedar language guide and the `cedar-policy` crate; OPA/Rego for the comparison |
| Strong isolation | gVisor architecture overview; rootless Podman |
| Agent security | OWASP Top 10 for LLM applications (prompt injection, excessive agency); benchmark suites such as AgentDojo for adversarial agent workflows |
| Protocols | MCP spec (tools and auth), ACP, A2A — where a broker can sit in each |

## 11. Risks

| Risk | Mitigation |
|---|---|
| A harness can't run without broad access (home config dirs, network) | Per-harness profile with the minimum extra paths, recorded in the support matrix; never fall back silently to "unconfined" |
| Too many approvals, so users click "approve" blindly | Measure approvals per task in R4; make policy templates and sensible defaults, not prompts for everything |
| User-namespace kernel bugs | Keep L4–L5 layers; evaluate a dedicated agent UID and gVisor in Phase 5 |
| Guard becomes a bottleneck or single point of failure | Fail closed; decisions cached per session for read-only checks; latency measured in R4 |
| Advisory vs enforced gets blurred in the UI | `enforcement` field is mandatory in the contract; P4 shows it on every decision |
