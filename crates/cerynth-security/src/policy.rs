//! Guardrail policies in Cedar, combined with per-session capabilities.
//!
//! The model maps directly onto Cedar's: principal = session, action =
//! capability kind, resource = target. Request data is in `context`:
//! `target`, `args`, `mode`, `origin`, `harness`, `is_child`.
//!
//! Final decision = strictest of (capability grant, Cedar):
//! - no matching capability            → deny (`no_matching_capability`)
//! - capability says deny              → deny (`policy_deny`)
//! - a Cedar `forbid` applies          → deny (`policy_deny`, that policy's `@id`)
//! - a determining permit has `@mode("approval")`, or the capability says approval
//!                                     → approval required
//! - otherwise                         → allow
//!
//! Cedar ignores policies whose evaluation errors. For a guardrail that would
//! mean a `forbid` silently not applying, so any evaluation error is a deny.

use std::str::FromStr;

use cedar_policy::{
    Authorizer, Context, Decision, Entities, EntityId, EntityTypeName, EntityUid, PolicySet, Request,
};

use crate::capability::{CapabilityKind, Mode};
use crate::decision::{ActionRequest, Reason, Verdict};
use crate::profile::{GuardSession, SessionMode};

/// The guardrails shipped by default (`packaging/guard/default.cedar`).
pub const DEFAULT_POLICIES: &str = include_str!("../../../packaging/guard/default.cedar");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyOutcome {
    pub verdict: Verdict,
    pub reason: Reason,
    pub policy_id: Option<String>,
}

pub struct PolicyEngine {
    policies: PolicySet,
    authorizer: Authorizer,
}

impl PolicyEngine {
    /// # Errors
    /// The policy text doesn't parse. guardd refuses to start (fails closed).
    pub fn new(text: &str) -> anyhow::Result<Self> {
        let policies = PolicySet::from_str(text).map_err(|e| anyhow::anyhow!("invalid Cedar policy: {e}"))?;
        Ok(Self { policies, authorizer: Authorizer::new() })
    }

    /// # Errors
    /// Never for the built-in policies; kept fallible for symmetry with `new`.
    pub fn with_defaults() -> anyhow::Result<Self> {
        Self::new(DEFAULT_POLICIES)
    }

    fn policy_label(&self, id: &cedar_policy::PolicyId) -> String {
        self.policies
            .policy(id)
            .and_then(|p| p.annotation("id").map(str::to_string))
            .unwrap_or_else(|| id.to_string())
    }

    fn uid(ty: &str, id: &str) -> anyhow::Result<EntityUid> {
        Ok(EntityUid::from_type_name_and_id(EntityTypeName::from_str(ty)?, EntityId::new(id)))
    }

    /// Evaluate one request for one session.
    #[must_use]
    pub fn evaluate(&self, session: &GuardSession, req: &ActionRequest) -> PolicyOutcome {
        let deny = |reason, policy_id: Option<String>| PolicyOutcome { verdict: Verdict::Deny, reason, policy_id };
        let kind = req.action.kind;
        if kind == CapabilityKind::Unknown {
            return deny(Reason::UnknownActionKind, None);
        }
        if session.mode != SessionMode::Managed && kind != CapabilityKind::SessionSpawn {
            // Guard can only broker for sessions it confines.
            return deny(Reason::SessionNotManaged, None);
        }

        let cap_mode = match session.capabilities.grant(kind, &req.action.target) {
            None => return deny(Reason::NoMatchingCapability, None),
            Some(Mode::Deny) => return deny(Reason::PolicyDeny, Some("capability".into())),
            Some(m) => m,
        };

        let cedar = match self.cedar(session, req) {
            Ok(c) => c,
            Err(e) => return deny(Reason::PolicyDeny, Some(format!("policy-error: {e}"))),
        };
        if let Some(outcome) = cedar {
            return outcome;
        }
        if cap_mode == Mode::Approval {
            return PolicyOutcome { verdict: Verdict::ApprovalRequired, reason: Reason::PolicyRequiresApproval, policy_id: Some("capability".into()) };
        }
        PolicyOutcome { verdict: Verdict::Allow, reason: Reason::PolicyAllow, policy_id: None }
    }

    /// `Ok(Some(outcome))` when Cedar denies or requires approval,
    /// `Ok(None)` when it simply allows.
    fn cedar(&self, session: &GuardSession, req: &ActionRequest) -> anyhow::Result<Option<PolicyOutcome>> {
        let principal = Self::uid("Cerynth::Session", &session.session_id.to_string())?;
        let action = Self::uid("Cerynth::Action", req.action.kind.as_str())?;
        let resource = Self::uid("Cerynth::Target", &req.action.target)?;
        let args = if req.action.args.is_object() { req.action.args.clone() } else { serde_json::json!({}) };
        let ctx = serde_json::json!({
            "target": req.action.target,
            "args": args,
            "mode": serde_json::to_value(session.mode)?,
            "origin": serde_json::to_value(req.context.origin)?,
            "harness": session.harness.clone().unwrap_or_default(),
            "is_child": session.parent_session_id.is_some(),
        });
        let context = Context::from_json_value(ctx, None)?;
        let request = Request::new(principal, action, resource, context, None)?;
        let resp = self.authorizer.is_authorized(&request, &self.policies, &Entities::empty());

        if let Some(err) = resp.diagnostics().errors().next() {
            return Ok(Some(PolicyOutcome { verdict: Verdict::Deny, reason: Reason::PolicyDeny, policy_id: Some(format!("policy-error: {err}")) }));
        }
        let determining: Vec<_> = resp.diagnostics().reason().collect();
        match resp.decision() {
            Decision::Deny => {
                let id = determining.first().map_or_else(|| "no-permit".to_string(), |id| self.policy_label(id));
                Ok(Some(PolicyOutcome { verdict: Verdict::Deny, reason: Reason::PolicyDeny, policy_id: Some(id) }))
            }
            Decision::Allow => {
                let approval = determining.iter().find(|id| {
                    self.policies.policy(id).and_then(|p| p.annotation("mode")) == Some("approval")
                });
                Ok(approval.map(|id| PolicyOutcome {
                    verdict: Verdict::ApprovalRequired,
                    reason: Reason::PolicyRequiresApproval,
                    policy_id: Some(self.policy_label(id)),
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Capability, CapabilitySet};
    use uuid::Uuid;

    fn session(caps: Vec<Capability>, child: bool) -> GuardSession {
        GuardSession {
            session_id: Uuid::new_v4(),
            parent_session_id: child.then(Uuid::new_v4),
            mode: SessionMode::Managed,
            workspace: "/tmp".into(),
            capabilities: CapabilitySet(caps),
            harness: Some("opencode".into()),
        }
    }
    fn push(s: &GuardSession, branch: &str, force: bool) -> ActionRequest {
        ActionRequest::new(s.session_id, CapabilityKind::GitPush, format!("origin:refs/heads/{branch}"), serde_json::json!({ "force": force }))
    }
    fn caps_push_all() -> Vec<Capability> {
        vec![
            Capability::new(CapabilityKind::GitPush, "origin:*", Mode::Allow),
            Capability::new(CapabilityKind::NetEgress, "api.anthropic.com:443", Mode::Allow),
        ]
    }

    #[test]
    fn default_policies_parse() {
        assert!(PolicyEngine::with_defaults().is_ok());
    }

    #[test]
    fn deny_by_default_without_capability() {
        let e = PolicyEngine::with_defaults().unwrap();
        let s = session(vec![], false);
        let o = e.evaluate(&s, &push(&s, "agent/x", false));
        assert_eq!((o.verdict, o.reason), (Verdict::Deny, Reason::NoMatchingCapability));
    }

    #[test]
    fn pushes_need_approval_by_default() {
        let e = PolicyEngine::with_defaults().unwrap();
        let s = session(caps_push_all(), false);
        let o = e.evaluate(&s, &push(&s, "agent/fix", false));
        assert_eq!(o.verdict, Verdict::ApprovalRequired);
        assert_eq!(o.policy_id.as_deref(), Some("git-push-needs-approval"));
    }

    #[test]
    fn guardrails_beat_capabilities() {
        let e = PolicyEngine::with_defaults().unwrap();
        let s = session(caps_push_all(), false);
        let force = e.evaluate(&s, &push(&s, "agent/fix", true));
        assert_eq!((force.verdict, force.policy_id.as_deref()), (Verdict::Deny, Some("no-force-push")));
        let main = e.evaluate(&s, &push(&s, "main", false));
        assert_eq!((main.verdict, main.policy_id.as_deref()), (Verdict::Deny, Some("protect-default-branches")));
        let child = session(caps_push_all(), true);
        let sub = e.evaluate(&child, &push(&child, "agent/fix", false));
        assert_eq!((sub.verdict, sub.policy_id.as_deref()), (Verdict::Deny, Some("subagents-cannot-push")));
    }

    #[test]
    fn plain_egress_is_allowed_when_granted() {
        let e = PolicyEngine::with_defaults().unwrap();
        let s = session(caps_push_all(), false);
        let req = ActionRequest::new(s.session_id, CapabilityKind::NetEgress, "api.anthropic.com:443", serde_json::Value::Null);
        assert_eq!(e.evaluate(&s, &req).verdict, Verdict::Allow);
        let other = ActionRequest::new(s.session_id, CapabilityKind::NetEgress, "evil.example:443", serde_json::Value::Null);
        assert_eq!(e.evaluate(&s, &other).reason, Reason::NoMatchingCapability);
    }

    #[test]
    fn evaluation_errors_fail_closed() {
        // `context.args.reason` doesn't exist → this forbid errors. Cedar would
        // skip it; we must deny instead.
        let e = PolicyEngine::new(
            r#"permit(principal, action, resource);
               @id("buggy") forbid(principal, action, resource) when { context.args.reason == "x" };"#,
        )
        .unwrap();
        let s = session(caps_push_all(), false);
        let req = ActionRequest::new(s.session_id, CapabilityKind::NetEgress, "api.anthropic.com:443", serde_json::json!({}));
        let o = e.evaluate(&s, &req);
        assert_eq!(o.verdict, Verdict::Deny);
        assert!(o.policy_id.unwrap().starts_with("policy-error"));
    }

    #[test]
    fn invalid_policy_text_is_rejected() {
        assert!(PolicyEngine::new("permit(principal action resource").is_err());
    }

    #[test]
    fn unmanaged_sessions_get_nothing_brokered() {
        let e = PolicyEngine::with_defaults().unwrap();
        let mut s = session(caps_push_all(), false);
        s.mode = SessionMode::Observed;
        assert_eq!(e.evaluate(&s, &push(&s, "agent/x", false)).reason, Reason::SessionNotManaged);
    }
}
