# P3-107 — Pin and test supported versions

**Status:** Partial  
**Task:** Authenticated cloud feature/version qualification  
**Branch:** `feat/person1-platform`  
**Audit date:** 2026-11-26

## What is implemented locally

- Reproducible offline inventory verifies all installed harness/backend/engine pins
- Local adapter/protocol regressions and evidence matrix exist
- `tools/runtime_version_inventory.py` — checks pinned local executables and isolated engine metadata without authentication
- Pinned versions: OpenCode 1.19.5, Hermes 0.23.1, Codex 0.167.0, Claude Code 2.1.312, Ollama 0.35.0, vLLM 0.25.0, SGLang 0.5.16, LMCache 0.5.6
- Protocol pins: agent API 1/v1alpha1 experimental, ACP 1, MCP 2025-11-25 initialization subset, A2A 0.3.0 supervised HTTP subset
- Host Claude CLI drift caught (2.1.312 → 2.1.326); older installed executable matches published hash; baseline selects it via `CERYNTH_CLAUDE_BINARY`

## Local check results

`tools/runtime_version_inventory.py` could not complete fully because pinned external binaries (OpenCode, Hermes, Ollama, etc.) are not installed at the expected paths under `artifacts/person3/tools/`. This is expected — these binaries must be installed separately by the owner. The inventory reports `all_local_pins_match` only when all binaries are present.

## What remains to close this task

Per the ledger, P3-107 requires:

1. **Authenticated cloud feature/version qualification** — exercise the selected pinned provider/harness versions against real accounts
2. **Publish supported operations, failures and unsupported features**
3. Full authenticated cloud feature compatibility remains unqualified until deferred cloud tests

Offline version inventory and local protocol checks already exist. Version probes have no external network.

## Next steps

- Install pinned binaries at expected paths or set `CERYNTH_*_BINARY` overrides
- Complete offline version inventory run
- Obtain authenticated cloud accounts for provider/version qualification
- Exercise pinned provider/harness versions against real accounts
- Publish supported operations, failures and unsupported features
- Update this file and the Person 3 task ledger

## Blocker

Cloud feature/version qualification requires authenticated provider accounts. Deferred until cloud setup is available.
