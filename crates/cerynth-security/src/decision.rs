//! `ActionRequest`, `SecurityDecision` and approvals, exactly as in
//! `docs/contracts/security-decision-v1alpha1.md`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::capability::CapabilityKind;

pub const SCHEMA: &str = "cerynth.guard/v1alpha1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub kind: CapabilityKind,
    pub target: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    #[default]
    Agent,
    User,
    System,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RequestContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default)]
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionRequest {
    pub schema: String,
    pub request_id: Uuid,
    pub session_id: Uuid,
    pub action: Action,
    #[serde(default)]
    pub context: RequestContext,
}

impl ActionRequest {
    #[must_use]
    pub fn new(session_id: Uuid, kind: CapabilityKind, target: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            schema: SCHEMA.into(),
            request_id: Uuid::new_v4(),
            session_id,
            action: Action { kind, target: target.into(), args },
            context: RequestContext::default(),
        }
    }

    /// SHA-256 over the canonical JSON of `{session_id, action}`. This is what
    /// an approval is bound to.
    #[must_use]
    pub fn digest(&self) -> String {
        let value = serde_json::json!({ "session_id": self.session_id, "action": self.action });
        format!("sha256:{}", hex::encode(Sha256::digest(canonical_json(&value))))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Allow,
    Deny,
    ApprovalRequired,
}

/// Stable reason codes. UI text and tests key off these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    PolicyAllow,
    PolicyDeny,
    PolicyRequiresApproval,
    NoMatchingCapability,
    CapabilityExceedsParent,
    SpawnLimitReached,
    ApprovalExpired,
    ApprovalMismatch,
    ApprovalDenied,
    ApprovalUnknown,
    SessionUnknown,
    SessionNotManaged,
    UnknownActionKind,
    CallerNotAuthorized,
    GuardUnavailable,
    BrokerFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Enforcement {
    /// The OS boundary makes the other outcome impossible.
    Enforced,
    /// Guard is relying on the harness to honour the decision.
    Advisory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalInfo {
    pub approval_id: Uuid,
    pub digest: String,
    pub summary: String,
    pub risk: Risk,
    /// Unix seconds.
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecurityDecision {
    pub schema: String,
    pub request_id: Uuid,
    pub session_id: Uuid,
    pub decision: Verdict,
    pub reason: Reason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    pub enforcement: Enforcement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalInfo>,
    /// Result of a brokered action that Guard executed itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Unix seconds.
    pub decided_at: u64,
}

impl SecurityDecision {
    #[must_use]
    pub fn new(req: &ActionRequest, decision: Verdict, reason: Reason, enforcement: Enforcement) -> Self {
        Self {
            schema: SCHEMA.into(),
            request_id: req.request_id,
            session_id: req.session_id,
            decision,
            reason,
            policy_id: None,
            enforcement,
            approval: None,
            result: None,
            decided_at: now_secs(),
        }
    }

    /// What a caller must assume when Guard can't be reached: deny.
    #[must_use]
    pub fn guard_unavailable(req: &ActionRequest) -> Self {
        Self::new(req, Verdict::Deny, Reason::GuardUnavailable, Enforcement::Enforced)
    }

    #[must_use]
    pub fn is_allowed(&self) -> bool {
        self.decision == Verdict::Allow
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalAnswer {
    Approve,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub schema: String,
    pub approval_id: Uuid,
    pub digest: String,
    pub response: ApprovalAnswer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responder: Option<String>,
}

/// Canonical JSON: object keys sorted (recursively), no whitespace.
///
/// Written out explicitly because serde_json's key order depends on a
/// crate feature (`preserve_order`) that any dependency in the build can
/// switch on; a digest must not change with that.
#[must_use]
pub fn canonical_json(value: &serde_json::Value) -> Vec<u8> {
    fn write(v: &serde_json::Value, out: &mut Vec<u8>) {
        match v {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push(b'{');
                for (i, k) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    out.extend(serde_json::to_vec(k).unwrap_or_default());
                    out.push(b':');
                    write(&map[k], out);
                }
                out.push(b'}');
            }
            serde_json::Value::Array(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write(item, out);
                }
                out.push(b']');
            }
            scalar => out.extend(serde_json::to_vec(scalar).unwrap_or_default()),
        }
    }
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

#[must_use]
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(session: Uuid, target: &str, force: bool) -> ActionRequest {
        ActionRequest::new(session, CapabilityKind::GitPush, target, serde_json::json!({ "force": force }))
    }

    #[test]
    fn digest_binds_session_and_exact_action() {
        let s = Uuid::new_v4();
        let a = push(s, "origin:refs/heads/agent/x", false);
        let mut same = push(s, "origin:refs/heads/agent/x", false);
        same.request_id = Uuid::new_v4(); // request id is not part of the operation
        assert_eq!(a.digest(), same.digest());
        assert_ne!(a.digest(), push(s, "origin:refs/heads/agent/x", true).digest(), "args change");
        assert_ne!(a.digest(), push(s, "origin:refs/heads/main", false).digest(), "target change");
        assert_ne!(a.digest(), push(Uuid::new_v4(), "origin:refs/heads/agent/x", false).digest(), "session change");
        assert!(a.digest().starts_with("sha256:") && a.digest().len() == 7 + 64);
    }

    #[test]
    fn digest_ignores_key_order() {
        let s = Uuid::new_v4();
        let a = ActionRequest::new(s, CapabilityKind::ToolExec, "x", serde_json::json!({"a":1,"b":2}));
        let b = ActionRequest::new(s, CapabilityKind::ToolExec, "x", serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap());
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn wire_format_matches_contract() {
        let req: ActionRequest = serde_json::from_str(
            r#"{"schema":"cerynth.guard/v1alpha1","request_id":"6f1c0c2e-6f53-4c1d-9b55-6a0d3a1e9a10",
                "session_id":"0d8f6b8a-2c1b-4a5e-8a0e-2f9a5c6d1e44",
                "action":{"kind":"git.push","target":"origin:refs/heads/agent/fix-login","args":{"force":false}},
                "context":{"harness":"opencode","origin":"agent"}}"#,
        )
        .unwrap();
        let d = SecurityDecision::new(&req, Verdict::ApprovalRequired, Reason::PolicyRequiresApproval, Enforcement::Enforced);
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["decision"], "approval_required");
        assert_eq!(v["reason"], "policy_requires_approval");
        assert_eq!(v["enforcement"], "enforced");
        assert_eq!(SecurityDecision::guard_unavailable(&req).decision, Verdict::Deny);
    }
}
