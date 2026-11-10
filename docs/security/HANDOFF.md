> Historical Claude handoff. Subsequent implementation and evidence are in VALIDATION-2026-11-10.md, runtime-integration.md and egress.md. The transport and fixed inference gateway described as missing below now exist.

# Cerynth Guard (Person 2): handoff & status

Last updated: 2026-11-10. Branch: `feat/guard-sandbox`.

This file tracks the whole Person 2 plan: what is **done and tested**, what is
**blocked** (and exactly what you need to supply), and the **exact next steps**.
The plan itself is `docs/security/README.md`; the threat model is
`docs/security/threat-model.md`; the contract is
`docs/contracts/security-decision-v1alpha1.md`.

**Project note:** CerynthOS is your own optimised custom OS. Guard is its
defensive agent-execution layer. Policy: prefer integrating a well-tested,
respected open-source component over writing our own, for every component.

---

## 1. What is DONE and verified

All of this builds, is `cargo fmt` clean, is clippy-clean (minus `pedantic`,
which the workspace sets to `warn`), and has passing tests.

### `crates/cerynth-security` — the Guard core library (22 unit tests)
| Module | What it does |
|---|---|
| `capability.rs` | Typed capability grants; deny-by-default matching; strictest-wins (`deny > approval > allow`); child-⊆-parent delegation check. |
| `decision.rs` | `ActionRequest` / `SecurityDecision` / approvals, matching the contract. `canonical_json` + SHA-256 digest binds an approval to one exact `{session, action}`. Guard-unavailable = deny. |
| `policy.rs` | Cedar policy engine. Final decision = strictest of (capability grant, Cedar). **Fails closed**: any Cedar evaluation error → deny; invalid policy text → guardd refuses to start. |
| `profile.rs` | Turns a session's capabilities into the exact `bwrap` + `guard-exec` command line. Default profile = workspace-only, no network, no GPU, cleared env, no `/run/cerynth`. |
| `confine.rs` | Landlock (L4) + seccomp (L5), applied in-process. seccomp denylist blocks `bpf`, `keyctl`, `mount`, `ptrace`, `io_uring`, `unshare`/`setns`, namespace-creating `clone`/`clone3`, etc. |
| `audit.rs` | Append-only, hash-chained JSONL audit log. Any edit/delete/reorder is detected by `verify`; a tampered log refuses further appends. |

Binaries (both build, both used by the boundary suite):
- `cerynth-guard-exec` — runs first inside the sandbox; applies Landlock+seccomp then execs the agent. **Fails closed** if Landlock can't enforce (override only via `CERYNTH_GUARD_ALLOW_PARTIAL_LANDLOCK=1`).
- `cerynth-guard-run` — host-side dev launcher: capabilities → sandbox.

### `services/cerynth-guardd` — the daemon library (7 unit tests)
| Module | What it does |
|---|---|
| `config.rs` | `guardd.toml` parsing with `deny_unknown_fields`; absolute-path and TTL validation; `build()` loads policy+vault+audit, failing closed. |
| `state.rs` | The `Guard` state: session registry, **delegation enforcement** (child must name a known parent, stay within its spawn limit, be a capability subset, and not push to remotes the parent lacks), decisions, single-use digest-bound expiring approvals, audit + event broadcast on every decision. |
| `vault.rs` | Credential store (one JSON file per secret). **Refuses to load** if the dir isn't `0700`/owned-by-us or any file is group/other-readable. `Debug` never prints secrets; `redact()` scrubs them from logs/events. |
| `git.rs` | Brokered `git push`: copies the one approved commit into a Guard-owned **bare mirror** (so the agent repo's hooks/config never run on the host), pushes from the mirror to a **registered** URL (never the agent's remote config), https-only, hooks off, prompts off, credential only in the child env. |
| `dispatch.rs` | **Transport-independent** request handling + caller authorization. `Caller::Session` can only `Decide`/`AwaitApproval` for its *own* session (naming another session = `caller_not_authorized`, never acts on it); `Caller::Trusted` gets the full API. This is the security-critical logic and it is unit-tested without any socket. |
| `main.rs` | Offline CLI: `check-config`, `verify-audit`. `serve` prints "not implemented" and points here. |

### Scripts, tests, docs (committed earlier, `6ed6feb`)
- `scripts/guard/guard-sandbox.sh` — unprivileged bwrap sandbox (prototype).
- `tests/security/sandbox-boundary.sh` — **34/34** through the Rust launcher (8 legit-work allows, 17 attack denies, L4/L5 layer checks, 6 host controls proving each attack really works outside the sandbox).
- `docs/security/{README.md, threat-model.md}`, `docs/contracts/security-decision-v1alpha1.md`, `packaging/guard/default.cedar`.

### Default guardrails (`packaging/guard/default.cedar`)
Deny-by-default; `git.push` needs approval; no force-push; protected
`main`/`master`; subagents can't push; host writes outside the workspace need
approval; children can't use credentials unless explicitly delegated.

---

## 2. What is BLOCKED and what I need from you

Two components were stopped **by a safety classifier** mid-write (twice each).
I did not retry them. Per your instruction, I isolated them and built
everything around them, leaving clean interfaces. **Both are the kind of
"integrate a respected open-source component" case you asked for.**

### BLOCKER A — IPC transport (the privileged socket server)
**What's missing:** the code that listens on `/run/cerynth/guard.sock` and the
per-session sockets, authenticates the peer via `SO_PEERCRED`, and feeds
requests into `dispatch::dispatch(guard, caller, req)`.

**Everything it would call is already written and tested** (`dispatch.rs`,
`state.rs`, the `Guard::events` broadcast channel). Only the socket plumbing
is absent.

**What you need to provide (one of):**
1. **Preferred — reuse Tokio's `UnixListener` + `tokio-util` `Framed`/`LinesCodec`** (both already respected, widely used). The server is ~120 lines:
   - main socket at `cfg.runtime_dir/guard.sock`, mode `0660`, group `cerynth-guard`;
   - peer UID via `getsockopt(SO_PEERCRED)` (use the `nix` crate, already a dep) → map to `Caller::Trusted` only if UID is root or in an allowlist (runtime + desktop users); otherwise reject;
   - one session socket per `RegisterSession` at `cfg.runtime_dir/sessions/<uuid>.sock`, bound into that sandbox, caller fixed to `Caller::Session(uuid)`;
   - `Subscribe` streams `guard.events.subscribe()`.
   The dispatch/authorization logic is done and tested, so this layer is "just" the socket I/O.
2. Or hand it to Person 3/Person 1 as the shared IPC layer (cerynthd already has a Unix-socket IPC pattern in `services/cerynthd` + `crates/cerynth-ipc` — reuse that framing so there's one IPC style in the repo).

**My recommendation:** option 1, mirroring the existing `cerynth-ipc` newline-JSON framing. I can write it if a future session isn't blocked; if it is, this is a self-contained task for you or P1.

### BLOCKER B — Egress gateway (cloud-API access from sandboxes)
**Decision you already made:** integrate an existing, reviewed gateway rather
than write a custom credential-injecting proxy. Good — that's the safer path.

**Current state:** sandboxes have **no network at all** (default-deny, enforced
by the net namespace). That is correct and safe; it just means cloud agents
(Claude Code, Codex) can't reach their APIs yet. Local-model and offline
coding tasks work today.

**What to integrate (candidates, all respected OSS):**
- **mitmproxy** (`--mode reverse`/`upstream`, addon scripts) — mature, scriptable, can enforce an allowlist and add auth headers outside the sandbox.
- **Envoy** or **Pomerium** — heavier, strong allowlisting/mTLS, good if you want policy-as-config.
- **Squid** with an allowlist + `peer` auth — classic, but credential injection is awkward.
- Simplest: a tiny per-session **allowlist-only** forward proxy that does **not** hold credentials; the agent keeps using its own key via the model provider, and Guard only restricts *which hosts* it can reach. (Loses "key never in the sandbox", keeps "default-deny + allowlist".)

**The integration boundary is already defined in code** so nothing else has to
change when you pick one:
- `profile.rs` has `EGRESS_PORT = 8099`, the `HTTP(S)_PROXY` env wiring, and `EGRESS_SOCKET_IN_SANDBOX`;
- `guard-exec` already forwards `127.0.0.1:8099` → a Unix socket if one is bound;
- `state.rs::launch_argv` currently passes `egress_socket: None` (no network). When the gateway exists, set it to `Some(guard.egress_socket(id))` and point the forwarder at the gateway's socket.

**What I need from you:** pick a gateway from the list (or name another). Then
either you wire it in, or I generate the per-session launch config and the
allowlist derived from each session's `net.egress` capabilities.

---

## 3. Status of every Person 2 requirement (from the plan)

| Plan item | Status |
|---|---|
| Threat model | ✅ v0 written, in review |
| `SecurityDecision` contract | ✅ v1alpha1, needs P1/P3/P4 sign-off |
| Sandbox: workspace rw, system ro, no home/net/control-plane, cleared env | ✅ |
| Landlock (L4) + seccomp (L5) | ✅ enforced, fail-closed |
| Capability model + deny-by-default + child⊆parent | ✅ tested |
| Policy engine (Cedar) | ✅ fails closed |
| Policy engine choice (Cedar vs OPA) | ⏳ Cedar chosen & implemented; the written decision record (`docs/security/decisions/`) is TODO |
| Approval service (single-use, digest-bound, expiring) | ✅ in `state.rs`, tested |
| Credential vault + redaction | ✅ tested |
| Brokered `git push` | ✅ implemented; **git-flow integration test blocked** (see §2 note) |
| Audit log (hash-chained) | ✅ tested |
| Session registry + delegation | ✅ tested |
| Caller authorization (agent can't approve/impersonate) | ✅ `dispatch.rs`, tested |
| Config validation | ✅ tested |
| CLI (`check-config`, `verify-audit`) | ✅ |
| **IPC socket server** | 🔴 BLOCKER A |
| **Egress gateway** | 🔴 BLOCKER B (pick an OSS one) |
| Boundary test suite | ✅ 34/34 |
| Adversarial regression suite (~50 attacks) | ⏳ TODO (Phase 2) |
| Broker-bypass tests | ⏳ TODO (Phase 3) |
| GPU/inference confinement tests | ⏳ TODO (needs P3 + real GPU) |
| R4 security benchmark | ⏳ TODO (Phase 3) |
| systemd unit + `cerynth-guard` group + packaging | ⏳ TODO (needs IPC first) |
| VM run of the boundary suite on kernel 7.1.0+ | ⏳ TODO (P2-07) — host passes 34/34; VM run pending |

---

## 4. Dependencies I added (all current, respected crates)
- `crates/cerynth-security`: `cedar-policy 4`, `landlock 0.4`, `seccompiler 0.5`, `sha2 0.10`, `hex 0.4`.
- `services/cerynth-guardd`: `cerynth-security` (path), `toml`, `nix`, `tokio`, plus `tempfile` (dev). (I removed `reqwest`/`httparse`/`bytes`/`futures-util` — they were only for the custom proxy I'm no longer writing.)

No `Cargo.lock` surprises; everything built from crates.io.

---

## 5. Exact next steps

### Immediate (you, no code): open the Sprint 1 PR
The committed work (`6ed6feb`) is ready for review. The new `cerynth-guardd`
work below is **not committed yet** (see §6). To open the PR for what's on the
branch now, or after committing §6:
```bash
cd ~/Projects/CerynthOS
git push -u origin feat/guard-sandbox
gh pr create --base main --head feat/guard-sandbox \
  --title "guard: Sprint 1 sandbox, security crate, daemon core, boundary tests" \
  --body-file docs/security/README.md
# Person 1 is the intended reviewer.
```

### Short term
1. **Pick the egress gateway** (§2 Blocker B) — this is the one real decision I need.
2. **IPC transport** (§2 Blocker A) — reuse `cerynth-ipc` framing + Tokio `UnixListener`. Self-contained ~120 lines; the logic it drives is done.
3. **Cedar-vs-OPA decision record** — write `docs/security/decisions/0001-cedar.md` (Cedar won: native Rust, in-process, analyzable, principal/action/resource maps cleanly).
4. **VM run (P2-07)** — run `tests/security/sandbox-boundary.sh` via `cerynth-guard-run` inside the Cerynth VM (kernel 7.1.0+) and record a support matrix. Host already passes 34/34.

### Phase 2+
5. Adversarial regression suite, broker-bypass suite, R4 benchmark, systemd packaging (`cerynth-guard` group, `0660` sockets), GPU confinement with P3.

---

## 6. Git state right now (nothing committed by me this session)
Uncommitted on `feat/guard-sandbox`:
- `services/cerynth-guardd/` (new crate: config, state, vault, git, dispatch, protocol, main, lib)
- `Cargo.toml` (added `services/cerynth-guardd` to the workspace)
- `Cargo.lock` (new deps)
- `docs/security/HANDOFF.md` (this file)
- `crates/cerynth-security/src/policy.rs`, `bin/cerynth-guard-exec.rs` (small clippy-clean edits)

Suggested commit (run yourself; I don't commit/push):
```bash
cd ~/Projects/CerynthOS
cargo fmt --all && cargo test -p cerynth-security -p cerynth-guardd
git add services/cerynth-guardd Cargo.toml Cargo.lock \
        docs/security/HANDOFF.md crates/cerynth-security
git commit -m "guard: daemon core (sessions, decisions, approvals, vault, brokered git, audit, config)

Transport-independent dispatch with caller authorization (agents can only
decide/await for their own session; cannot approve or impersonate).
Single-use digest-bound approvals; hash-chained audit; delegation enforced
(child must be a capability subset of a known parent). Credential vault
refuses loose permissions and redacts secrets. Brokered git push via a
Guard-owned mirror to registered remotes only, https-only, hooks off.

IPC socket server and egress gateway are left as documented interfaces
(docs/security/HANDOFF.md): both are 'integrate reviewed OSS' tasks.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## 7. How to verify everything I did (copy-paste)
```bash
cd ~/Projects/CerynthOS
cargo build -p cerynth-security -p cerynth-guardd
cargo test  -p cerynth-security -p cerynth-guardd         # 22 + 7 pass
cargo clippy -p cerynth-security -p cerynth-guardd --all-targets -- -A clippy::pedantic   # clean
tests/security/sandbox-boundary.sh                                           # 25/25 (prototype)
GUARD_LAUNCHER=$PWD/target/debug/cerynth-guard-run tests/security/sandbox-boundary.sh  # 34/34 (Landlock+seccomp)

# Try the daemon CLI:
cargo run -p cerynth-guardd -- verify-audit /nonexistent   # handles missing log
# Try a sandboxed shell (workspace-only, no network):
cargo build -p cerynth-security
target/debug/cerynth-guard-run --workspace /tmp -- bash
```

## 8. One decision I need from you
**Which egress gateway** (§2 Blocker B)? My recommendation: **mitmproxy in an
allowlist addon** for the first alpha (scriptable, can add auth headers outside
the sandbox so keys stay out), revisiting Envoy/Pomerium if you want
policy-as-config later. Reply with your pick and I'll produce the per-session
integration (launch config + allowlist derived from `net.egress` capabilities),
or hand it to you as a spec.
