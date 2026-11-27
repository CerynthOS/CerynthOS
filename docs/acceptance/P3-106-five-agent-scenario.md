# P3-106 — Run the five-agent local/cloud scenario

**Status:** Pending  
**Task:** The actual reference run  
**Branch:** `feat/person1-platform`  
**Audit date:** 2026-11-26

## What is implemented locally

- Parent/child identities and relationships (P3-035)
- Task assignment/delegation API (P3-036)
- Bounded child creation with deadlines, max children, reservations (P3-037)
- Messaging and structured results (P3-040)
- Graphs/dependencies with unresolved dependency blocking (P3-041)
- Git worktrees/workspace mapping (P3-042)
- Cancellation/deadlines/bounded retries (P3-044)
- Concurrent agents/termination/workspace tests (P3-046)
- Shared managed-domain reservations implemented (P3-087)

## What remains to close this task

Per the ledger, P3-106 requires:

1. **Dedicated authorized cloud/harness configuration**
2. **The actual reference run** — two local Hermes, two cloud coding and one Claude Code sessions
3. **Available hardware** with authorized models/accounts and separate GPU-tool admission
4. **Retain resource, outcome and cleanup evidence**
5. **Document deviations** from illustrative hardware

Shared managed-domain reservations are implemented; unregistered backend processes and bypassing workloads remain outside their coverage.

## Next steps

- Obtain dedicated cloud/harness configuration with authorized models/accounts
- Configure two local Hermes sessions, two cloud coding sessions, one Claude Code session
- Run the reference scenario on available hardware
- Retain resource, outcome and cleanup evidence
- Document any deviations from illustrative hardware
- Update this file and the Person 3 task ledger

## Blocker

Cannot be completed without dedicated cloud configuration and authorized accounts. Deferred until cloud setup is available.
