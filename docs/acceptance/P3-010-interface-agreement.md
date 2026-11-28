# P3-010 — Agree interfaces with Persons 1, 2 and 4

**Status:** Pending  
**Task:** Actual coordinated interface agreement with Persons 1, 2 and 4  
**Branch:** `feat/person1-platform`  
**Audit date:** 2026-11-26

## What is implemented locally

The candidate interfaces and review packet are documented in
`docs/contracts/person1-interface-review.md`. The content-addressed protocol
candidate is committed at
`docs/contracts/schemas/agent-protocol-v1alpha1.manifest.json`. The Person 1
platform library is pinned at commit `648c149` with source hashes preserved in
`crates/cerynth-platform-resources/PROVENANCE.json`.

Automated evidence available:
- `cargo test --locked --workspace` — all tests pass
- `scripts/check-agent-contracts.sh` — request, session and protocol manifest match Rust contracts
- `tools/contract_compatibility_smoke.py` — contract pinning enforced (wrong pin rejected before mutation, matching pin accepted)

## What remains to close this task

Per the ledger, P3-010 requires:

1. **Responsible maintainers review** the current schema/fingerprint, resource/security/event interfaces
2. **Record real decisions with identities and evidence** — reviewer identity, candidate fingerprint, scope, decision and evidence must be recorded
3. **Team approval** from Persons 1, 2 and 4 — no approval has been claimed or fabricated

No messages have been sent to teammates. A local test, type or proposed contract is not substituted for their review.

## Next steps

- Share `docs/contracts/person1-interface-review.md` and the protocol manifest with Persons 1, 2 and 4
- Collect reviewer identities, candidate fingerprint adoption, scope decisions and evidence
- Record decisions in this file or the review packet
- Once all three reviewers sign off, update the Person 3 task ledger to mark P3-010 implemented

## Blocker

Cannot be completed without actual teammate participation and approval. This task is deferred until team review is scheduled.
