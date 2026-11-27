# P3-105 — Validate the mixed-harness workflow

**Status:** Pending  
**Task:** Authenticated local/cloud mixed execution, including Claude Code  
**Branch:** `feat/person1-platform`  
**Audit date:** 2026-11-26

## What is implemented locally

- Local harnesses exercised separately: OpenCode 1.19.5, Hermes 0.23.1, Codex 0.167.0
- Cross-harness routing: real OpenCode/Hermes parent-child copy workflow succeeds in separate worktrees with one authenticated inference queue (P3-095)
- A2A 0.3 supervised HTTP subset gateway implemented (P3-096)
- Optional remote execution prototype with SSH relay (P3-097)
- Claude Code 2.1.312 detected via `runtime_version_inventory.py`; version/recipe only, cloud execution unvalidated
- `CERYNTH_CLAUDE_BINARY` override honored for Claude baseline selection

## What remains to close this task

Per the ledger, P3-105 requires:

1. **Authenticated local/cloud mixed execution** — supply authorized harness/account configuration
2. **Including Claude Code** — authenticated Claude Code cloud workflow
3. **Run and independently verify** the actual mixed workflow, cancellations and cleanup
4. **Retain evidence** of the mixed execution

Separate local harness tests do not close this task. No cloud task context was transmitted in prior tests.

## Next steps

- Obtain authorized harness/account configuration for cloud providers including Claude Code
- Configure dedicated credential environment variables
- Run mixed local/cloud workflow with cancellations and cleanup verification
- Retain sanitized evidence
- Update this file and the Person 3 task ledger

## Blocker

Cannot be completed without authenticated cloud configuration and dedicated harness accounts. Deferred until cloud setup is available.
