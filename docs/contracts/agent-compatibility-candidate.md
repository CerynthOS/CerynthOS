# Content-addressed agent API candidate

The owner operation `protocol` returns the current API version, contract ID,
feature versions and `candidate_frozen_pending_team_review` status. The contract
ID hashes a recursively canonicalized request/session schema bundle plus an
explicit behavior revision and pinned resource/Guard contract identities.
Behavior-only changes must update that revision even when schemas are unchanged.
An ID is a compatibility pin, not a signature or a team approval record.

The candidate manifest is now committed as
`schemas/agent-protocol-v1alpha1.manifest.json` and exported by
`cerynth-agent-schema --manifest`. The existing CI contract check compares it
with the runtime alongside both JSON schemas. A behavior-only revision or new
feature therefore requires an explicit, reviewable manifest change. The
November 20 revision advertises `shared_resource_authority: 1`; older pins
correctly fail before mutation. Shared API review remains outstanding.

Owner clients using `cerynth_agentd::call` can set
`CERYNTH_AGENT_CONTRACT=<64-character SHA-256>` to include the pin as optional
`expected_contract` in the request frame. The server checks it in the same
dispatch as the operation, before mutation. A restart between discovery and a
write cannot bypass the check. Raw Unix clients can include the same optional
field; existing clients that omit it continue to work without a pin. This is
wire compatibility, not source compatibility for old Rust struct literals.

The fixture `crates/cerynth-agent-api/tests/fixtures/legacy-2d8b5d4.json` was
generated with the actual Rust types from commit `2d8b5d4`, including source
hashes. Tests parse and round-trip those old requests and the session record,
preserving unknown tool outcomes and absent optional resource/residency fields.
The generator is reproducible with `tools/generate_legacy_agent_fixture.py`.
New nonempty Guard remote mappings are a new feature: old strict clients must
negotiate/update before consuming sessions that use them. This is not a blanket
claim that every old client understands new session features.

The live `tools/contract_compatibility_smoke.py` checks that an incorrect pin
cannot create a session through either the Rust client or a direct raw frame,
a matching pin can, and the global proposal cannot
apply allocations from incomplete instance-only accounting.

P3-099 remains partial until coordinated compatibility review and stabilization.
No Person 1/2/4 sign-off, production-stable API or release qualification is
claimed by this candidate.
