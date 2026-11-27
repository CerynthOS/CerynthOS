# P3-085 — Evaluate consented local/cloud routing

**Status:** Partial  
**Task:** Authorized live cloud-routing experiment  
**Branch:** `feat/person1-platform`  
**Audit date:** 2026-11-26

## What is implemented locally

- `tools/cloud_routing_smoke.py` — ready for a selected HTTPS provider, supported model and dedicated credential variable
- Explicit consent enforced: non-consensual remote route configuration is rejected; session registration without `allow_cloud: true` is rejected
- `tools/cloud_routing_smoke.py` performs one bounded public challenge, verifies actual response and usage, then revokes the grant
- Credentials, response bodies and provider error bodies are excluded from reports
- Cloud-consent policy tests pass locally (3 cloud-consent policy regressions in release qualification)
- `CERYNTH_CLAUDE_BINARY` override honored for Claude 2.1.312 baseline selection

## What remains to close this task

Per the ledger, P3-085 requires:

1. **Owner-selected provider endpoint** (HTTPS, no credentials/query/fragment in URL)
2. **Dedicated credential environment-variable reference** (must match `CERYNTH_[A-Z0-9_]{1,119}`, not an ambient provider credential)
3. **Authorized account/model** for the provider
4. **Explicit session/privacy consent**
5. **Run the experiment** — one live cloud request through real `cerynth-inferenced`, verify response, revoke grant, verify revoked credential rejected
6. **Retain outcomes** — sanitized report to `artifacts/person3/cloud-routing-smoke.json`

## Next steps

- Owner selects provider endpoint and dedicated credential variable name (e.g., `CERYNTH_CLOUD_CREDENTIAL`)
- Configure the credential in the environment (never committed to the repository)
- Run:
  ```
  python3 tools/cloud_routing_smoke.py \
    --endpoint https://<provider-endpoint> \
    --model <supported-model> \
    --credential-env CERYNTH_CLOUD_CREDENTIAL \
    --consent
  ```
- Retain the sanitized output report
- Update this file and the Person 3 task ledger

## Blocker

Cannot be completed without dedicated cloud configuration. The audit states: "external physical, signing and review inputs cannot currently be supplied" and "cloud setup: provide endpoint and credential environment-variable names, not secret values." No live cloud success is claimed by current policy-only checks. Cloud requests remain at zero.
