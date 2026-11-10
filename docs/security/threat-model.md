# Cerynth Guard threat model (v0)

Status: **draft for Sprint 1 review** (Nov 9–15, 2026). Owner: Person 2.
Reviewers: Person 1 (platform boundaries), Person 3 (runtime and harnesses).

This document says what Cerynth Guard protects, from whom, and which promises
it does **not** make. Every Guard feature and test should trace back to a
threat listed here.

## 1. The core assumption

> **Treat every agent as potentially hostile.**

An agent can be manipulated by a web page, a poisoned README, a tool result or
a malicious MCP server (prompt injection). The model can also simply be wrong.
Guard does not try to detect bad intent with another LLM. It enforces limits at
the execution layer, so that a fully compromised agent still cannot do more
than its session was granted.

A defence that works only when the agent cooperates is not a defence.

## 2. What we protect (assets)

| ID | Asset | Examples | Why it matters |
|---|---|---|---|
| A1 | User secrets | `~/.ssh`, `~/.config/gh`, browser profiles, password stores, `.env` files outside the workspace | Theft gives the attacker the user's identity |
| A2 | Provider credentials | Anthropic/OpenAI/other API keys, cloud CLI credentials | Money, data exfiltration, account takeover |
| A3 | Host integrity | `/usr`, `/etc`, `/boot`, systemd units, the kernel | Persistence, full compromise |
| A4 | Cerynth control plane | `/run/cerynth/*.sock`, `/var/lib/cerynth`, `/etc/cerynth` | An agent that can drive cerynthd can change its own limits |
| A5 | Other sessions | Other agents' workspaces, memory, processes | Cross-agent data theft or sabotage |
| A6 | The user's repositories and remote accounts | Git remotes, issue trackers, email | Destructive or unauthorised external actions |
| A7 | The audit trail | Guard decision and approval log | Without it, nothing can be investigated |
| A8 | Machine availability | CPU, RAM, disk, GPU | A runaway agent can make the machine unusable (shared with Person 1) |

## 3. Who we defend against (adversaries)

| ID | Adversary | Capability we assume |
|---|---|---|
| T1 | **Compromised agent** (prompt-injected or misbehaving model) | Runs arbitrary code inside its sandbox, with any arguments, as fast as it likes |
| T2 | **Malicious content** | Controls web pages, files, issues or tool output the agent reads |
| T3 | **Malicious tool or MCP server** | Returns crafted output; may try to talk to other services |
| T4 | **Rogue subagent** | Tries to get more capabilities than its parent granted |
| T5 | **Other local user** (multi-user machines) | Ordinary unprivileged account on the same host |

Out of scope for v0 (see §7): a malicious kernel, physical access, a
compromised harness binary installed by the user on purpose, and the cloud
provider itself.

## 4. Trust boundaries

```
 trusted                                     untrusted
 ───────────────────────────────────────┬────────────────────────────────────
 kernel, systemd                         │ agent process tree (harness, model
 cerynthd (P1), cerynth-guardd (P2)      │   client, shells, tools, scripts)
 cerynth-agentd (P3) — launches only     │ content the agent reads
 the user, via the desktop (P4)          │ MCP servers / tools the agent runs
 Guard policy files (root-owned)         │ child sessions
```

The boundary is the **sandbox edge**. Anything crossing it, such as a file
outside the workspace, a network connection, a credential, a Cerynth API call
or a child session, goes through a Guard decision. If something can cross
without a decision, that is a bug.

## 5. Threats and planned controls

| ID | Threat | Adversary | Asset | Control | Phase |
|---|---|---|---|---|---|
| S1 | Read secrets outside the workspace | T1, T2 | A1 | Mount namespace shows only the workspace and read-only system dirs; Landlock as a second layer | **P0 ✅ prototype** |
| S2 | Steal API keys from the environment | T1 | A2 | Cleared environment with an allowlist; credential broker injects keys outside the sandbox | P0 ✅ env / P2 broker |
| S3 | Exfiltrate data over the network | T1, T2 | A1, A2, A5 | No network by default; egress only through a Guard proxy with a per-session allowlist | P0 ✅ deny / P2 proxy |
| S4 | Modify the host (persistence) | T1 | A3 | Read-only system mounts, no capabilities, no setuid inside the user namespace, seccomp filter | P0 ✅ / P2 seccomp |
| S5 | Drive the Cerynth control plane | T1 | A4 | `/run/cerynth` and `/var/lib/cerynth` not mounted; sockets authenticate peers (`SO_PEERCRED`); privileged sockets are root-only | P0 ✅ / P1 auth |
| S6 | Attack other sessions | T1, T4 | A5 | One sandbox per session, private PID namespace, separate workspaces / Git worktrees | P0 ✅ / P2 |
| S7 | Destructive external action (force-push, delete repo, send email) | T1, T2 | A6 | Action broker holds the credential; policy says allow / deny / approve; approval bound to the exact operation | P1–P2 |
| S8 | Approval substitution (approve A, execute B) | T1 | A6 | Approval binds a digest of action + target + arguments; the broker executes it; single-use with expiry | P2 |
| S9 | Subagent privilege escalation | T4 | all | Child capabilities ⊆ parent capabilities, enforced by Guard rather than the harness | P2 |
| S10 | Tool/MCP output steering the agent | T2, T3 | A6 | Treated as untrusted; risky actions still need policy or approval. Provenance and taint tracking are research (R4) | P5–P6 |
| S11 | Tamper with or flood the audit log | T1 | A7 | Log written by guardd outside the sandbox; append-only and hash-chained; rate limits | P1–P2 |
| S12 | Escape through the terminal (TIOCSTI keystroke injection) | T1 | A3 | `--new-session` (no controlling TTY), plus kernel `dev.tty.legacy_tiocsti=0` where available | P0 ✅ |
| S13 | Resource exhaustion (fork bomb, disk fill) | T1 | A8 | Person 1's cgroup limits (pids, memory, I/O); tmpfs size limits | P1 (with P1) |
| S14 | Kernel attack surface through rarely used syscalls | T1 | A3 | seccomp denylist (`bpf`, `keyctl`, `ptrace` outside the sandbox, `mount`, `io_uring`, `userfaultfd`, …) | P2 |

## 6. Agent modes and what Guard can honestly promise

| Mode | Launched by | Guard guarantee |
|---|---|---|
| **Managed** | Cerynth (agentd + guardd) | Full: everything in §5 that is implemented |
| **Integrated** | The user, connected through a harness API | Partial: only what the harness lets Guard intercept |
| **Observed** | The user, in a normal terminal | **None.** Cerynth can watch it but cannot protect against it |

The desktop must show these differently (Person 4). Guard must never show
"protected" for a session it does not actually confine.

## 7. Explicit non-goals for v0

- Detecting prompt injection with a classifier or LLM. Guard enforces limits; it does not try to read intent.
- Protecting against the kernel, firmware, or someone with physical access.
- Confining harness binaries the user runs outside Cerynth (Observed mode).
- Hard GPU memory isolation between sessions (hardware-dependent; see Person 3's capability labels).
- Protecting data that is **inside** the granted workspace from that same workspace's agent. If a session may read it, it can leak it through any channel it is allowed to use.

## 8. Known gaps in the Sprint 1 prototype

These are tracked for Sprint 2 onwards:

1. `/etc` is mounted read-only in full. Files there that are readable by the user, which can include internal hostnames or VPN config, are visible to the agent. Mount only what tools need.
2. There is no Landlock or seccomp layer yet. The mount namespace is currently the only filesystem boundary.
3. No egress proxy yet, so network access is all or nothing (`--net`).
4. The sandbox runs as the invoking user's UID mapped into a user namespace. A kernel user-namespace bug would expose that user's files. A dedicated per-agent UID is a Phase 2 option.
5. There are no resource limits until Person 1's scopes exist.

## 9. Review checklist

- [ ] P1: are the control-plane assets (A4) and their socket permissions right?
- [ ] P3: can every planned harness run inside the default profile (no network, workspace only)? What does each one need?
- [ ] P4: can the desktop show the three agent modes distinctly?
- [ ] Everyone: anything we protect that is missing from §2?
