# P3-099 — Stabilize shared APIs and IPC

**Status:** Partial  
**Task:** Coordinated compatibility review and stable-release acceptance  
**Branch:** `feat/person1-platform`  
**Audit date:** 2026-11-26

## What is implemented locally

- Content-addressed candidate contract at `docs/contracts/schemas/agent-protocol-v1alpha1.manifest.json`
- Exported by `cerynth-agent-schema --manifest`
- CI checks manifest against runtime alongside both JSON schemas (`scripts/check-agent-contracts.sh`)
- Optional server-checked request pinning before mutation via `CERYNTH_AGENT_CONTRACT=<64-character SHA-256>`
- Atomic pin check with dispatch (no daemon-restart bypass gap)
- Historical Rust round trips preserved via `crates/cerynth-agent-api/tests/fixtures/legacy-2d8b5d4.json`
- Behavior revision and feature fingerprint changed intentionally; old pins fail before mutation
- Unpinned legacy frames continue to work (wire compatibility)

## Local check results

- `scripts/check-agent-contracts.sh` — PASS: Agent request, session and protocol manifest match Rust contracts
- `tools/contract_compatibility_smoke.py` — PASS:
  - Wrong contract rejected before mutation: true
  - Matching contract created session: true
  - Contract pin in mutating request checked by server: true
  - Raw wrong pin rejected without registry mutation: true
  - Instance-only global proposal queues without actuation: true
  - Team approval claimed: false
  - Cloud requests: 0

Contract ID observed: `b9ef3f0a367d199e86c08d69e4cc0a9b00e6f630e01c74ca9418f537c4b9e7fa`

## What remains to close this task

Per the ledger, P3-099 requires:

1. **Coordinated compatibility review** — review/adopt the exact candidate contract, supported clients and behavior
2. **Resolve review findings** — address any issues raised during coordinated review
3. **Record stabilization acceptance** — document that the shared API is stabilized for release

Hashes, historical round trips and CI drift checks already exist. The November 20 revision advertises `shared_resource_authority: 1`; older pins correctly fail before mutation.

## Next steps

- Distribute the contract candidate and manifest to stakeholders for review
- Collect review findings and resolve them
- Record stabilization acceptance with reviewer identities and decisions
- Update the manifest revision if behavior-only changes are needed
- Once coordinated review and stabilization acceptance are recorded, update the Person 3 task ledger

## Blocker

Coordinated compatibility review and stable-release acceptance require team participation. Cannot be completed unilaterally.
