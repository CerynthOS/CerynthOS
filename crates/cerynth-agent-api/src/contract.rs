//! Content-addressed candidate contract; consumers can pin before any mutation.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn canonical(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, canonical(v)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
        scalar => scalar,
    }
}

/// The schema fingerprint changes whenever the accepted request/session shape
/// changes. It is not a claim of teammate sign-off or release qualification.
///
/// # Panics
/// Panics if serialization of a built-in JSON schema fails.
#[must_use]
pub fn manifest() -> Value {
    let request = canonical(
        serde_json::to_value(schemars::schema_for!(crate::Request)).expect("schema serializes"),
    );
    let session = canonical(
        serde_json::to_value(schemars::schema_for!(crate::Session)).expect("schema serializes"),
    );
    let schemas = json!({"request":request,"session":session,"behavior_revision":"2026-11-19",
        "platform_budget":1,"guard_revision":"e592fa47797179ad24a99fc76143be48191a7649"});
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical(schemas)).expect("schema bundle serializes"))
    );
    json!({"version":crate::API_VERSION,"contract_id":digest,"status":"candidate_frozen_pending_team_review",
        "session_specification":"v1alpha1","fingerprint_algorithm":"sha256-canonical-schema-bundle",
        "features":{"platform_resource_budget":1,"tool_effect_deduplication":1,"global_resource_plan":1,"atomic_contract_pin":1},
        "team_approval_claimed":false})
}
