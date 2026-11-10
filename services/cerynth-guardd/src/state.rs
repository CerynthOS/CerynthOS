//! Guard's state and decision logic: sessions, approvals, audit, events.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cerynth_security::audit::AuditLog;
use cerynth_security::decision::now_secs;
use cerynth_security::policy::PolicyEngine;
use cerynth_security::{
    ActionRequest, ApprovalAnswer, ApprovalInfo, ApprovalResponse, CapabilityKind, Enforcement,
    HostPaths, Mode, Reason, Risk, SandboxProfile, SecurityDecision, SessionMode, Verdict,
};
use serde_json::json;
use tokio::sync::{Notify, broadcast};
use uuid::Uuid;

use crate::git;
use crate::protocol::{PendingView, Registration};
use crate::vault::Vault;

pub struct Config {
    pub endpoints: std::collections::BTreeMap<String, crate::egress::Endpoint>,
    pub runtime_dir: PathBuf,
    pub mirror_dir: PathBuf,
    pub guard_exec: PathBuf,
    pub approval_ttl: Duration,
    pub allow_file_remotes: bool,
    pub policy_source: String,
}

struct SessionEntry {
    reg: Registration,
    children: HashSet<Uuid>,
}

#[derive(Clone)]
enum PendingState {
    Waiting,
    Executing,
    // Boxed: the resolved decision is far larger than `Waiting`, and most
    // pending entries are waiting.
    Resolved(Box<SecurityDecision>),
}

struct Pending {
    request: ActionRequest,
    info: ApprovalInfo,
    state: PendingState,
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<Uuid, SessionEntry>,
    approvals: HashMap<Uuid, Pending>,
}

pub struct Guard {
    pub cfg: Config,
    policy: PolicyEngine,
    pub vault: Vault,
    audit: Mutex<AuditLog>,
    inner: Mutex<Inner>,
    approval_notify: Notify,
    pub events: broadcast::Sender<serde_json::Value>,
}

fn risk_of(kind: CapabilityKind) -> Risk {
    match kind {
        CapabilityKind::GitPush | CapabilityKind::CredentialUse | CapabilityKind::FsWrite => {
            Risk::High
        }
        CapabilityKind::NetEgress | CapabilityKind::GpuUse | CapabilityKind::SessionSpawn => {
            Risk::Medium
        }
        _ => Risk::Low,
    }
}

fn summary(req: &ActionRequest) -> String {
    match req.action.kind {
        CapabilityKind::GitPush => {
            let commit = req
                .action
                .args
                .get("commit")
                .and_then(|c| c.as_str())
                .unwrap_or("?");
            format!(
                "Push commit {} to {}",
                commit.chars().take(12).collect::<String>(),
                req.action.target
            )
        }
        CapabilityKind::NetEgress => format!("Connect to {}", req.action.target),
        CapabilityKind::FsWrite => format!("Write to {}", req.action.target),
        CapabilityKind::CredentialUse => format!("Use credential {}", req.action.target),
        k => format!("{} {}", k.as_str(), req.action.target),
    }
}

impl Guard {
    /// # Errors
    /// The audit log can't be opened or doesn't verify.
    pub fn new(
        cfg: Config,
        policy: PolicyEngine,
        vault: Vault,
        mut audit: AuditLog,
    ) -> anyhow::Result<Arc<Self>> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(cfg.runtime_dir.join("sessions"))?;
        std::fs::create_dir_all(&cfg.mirror_dir)?;
        // Sessions are not yet persisted across restarts. Recover the audit
        // history by explicitly invalidating unresolved approvals, never by
        // silently replaying an operation with a possible external effect.
        let mut unresolved = HashMap::<String, bool>::new();
        use std::io::BufRead as _;
        for line in std::io::BufReader::new(std::fs::File::open(audit.path())?).lines() {
            let record: cerynth_security::audit::AuditRecord = serde_json::from_str(&line?)?;
            if let Some(id) = record.data.get("approval_id").and_then(|v| v.as_str()) {
                match record.event.as_str() {
                    "guard.approval.requested" => {
                        unresolved.insert(id.to_owned(), false);
                    }
                    "guard.approval.claimed" => {
                        if let Some(claimed) = unresolved.get_mut(id) {
                            *claimed = true;
                        }
                    }
                    "guard.approval.resolved" | "guard.approval.invalidated_restart" => {
                        unresolved.remove(id);
                    }
                    _ => {}
                }
            }
        }
        for (id, claimed) in unresolved {
            audit.append("guard.approval.invalidated_restart", json!({
                "approval_id": id,
                "outcome": if claimed { "indeterminate_do_not_retry_automatically" } else { "cancelled" }
            }))?;
        }
        let (events, _) = broadcast::channel(1024);
        Ok(Arc::new(Self {
            cfg,
            policy,
            vault,
            audit: Mutex::new(audit),
            inner: Mutex::new(Inner::default()),
            approval_notify: Notify::new(),
            events,
        }))
    }

    /// Audit and broadcast. Audit failures are fatal for the decision path:
    /// callers turn them into deny.
    pub(crate) fn record(&self, event: &str, data: serde_json::Value) -> anyhow::Result<()> {
        fn redact(value: &mut serde_json::Value, vault: &Vault) {
            match value {
                serde_json::Value::String(s) => *s = vault.redact(s),
                serde_json::Value::Array(items) => {
                    for item in items {
                        redact(item, vault);
                    }
                }
                serde_json::Value::Object(items) => {
                    for item in items.values_mut() {
                        redact(item, vault);
                    }
                }
                _ => {}
            }
        }
        let mut data = data;
        redact(&mut data, &self.vault);
        self.audit
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?
            .append(event, data.clone())?;
        let _ = self
            .events
            .send(json!({ "type": event, "ts": now_secs(), "data": data }));
        Ok(())
    }

    pub fn session_socket(&self, id: Uuid) -> PathBuf {
        self.cfg
            .runtime_dir
            .join("sessions")
            .join(format!("{id}.sock"))
    }
    pub fn has_session(&self, id: Uuid) -> bool {
        self.inner
            .lock()
            .is_ok_and(|i| i.sessions.contains_key(&id))
    }

    /// Register a session. Children must name a known parent, stay within its
    /// spawn limit, and ask for a subset of its capabilities.
    ///
    /// # Errors
    /// A `Reason` code and message.
    pub fn register(&self, reg: Registration) -> Result<(), (Reason, String)> {
        let id = reg.session.session_id;
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| (Reason::GuardUnavailable, "state lock".into()))?;
            if inner.sessions.contains_key(&id) {
                return Err((
                    Reason::PolicyDeny,
                    format!("session {id} already registered"),
                ));
            }
            if let Some(parent_id) = reg.session.parent_session_id {
                let parent = inner.sessions.get(&parent_id).ok_or((
                    Reason::SessionUnknown,
                    format!("parent {parent_id} unknown"),
                ))?;
                if parent
                    .reg
                    .session
                    .capabilities
                    .grant(CapabilityKind::SessionSpawn, "child")
                    != Some(Mode::Allow)
                {
                    return Err((
                        Reason::NoMatchingCapability,
                        "parent may not spawn sessions".into(),
                    ));
                }
                if parent.children.len() >= parent.reg.session.capabilities.spawn_limit() as usize {
                    return Err((
                        Reason::SpawnLimitReached,
                        format!("parent already has {} children", parent.children.len()),
                    ));
                }
                if let Err(c) = parent
                    .reg
                    .session
                    .capabilities
                    .check_subset(&reg.session.capabilities)
                {
                    return Err((
                        Reason::CapabilityExceedsParent,
                        format!(
                            "child asks for {} {} beyond its parent",
                            c.kind.as_str(),
                            c.target
                        ),
                    ));
                }
                if parent.reg.session.mode != SessionMode::Managed
                    || reg.session.mode != SessionMode::Managed
                {
                    return Err((
                        Reason::SessionNotManaged,
                        "only managed sessions can delegate".into(),
                    ));
                }
                // Remotes too: a child can't push anywhere its parent couldn't.
                for (name, url) in &reg.remotes {
                    if parent.reg.remotes.get(name) != Some(url) {
                        return Err((
                            Reason::CapabilityExceedsParent,
                            format!("remote {name} not available to the parent"),
                        ));
                    }
                }
            }
            if let Err(e) = SandboxProfile::for_session(&reg.session) {
                return Err((Reason::PolicyDeny, e));
            }
            if let Some(parent_id) = reg.session.parent_session_id {
                if let Some(p) = inner.sessions.get_mut(&parent_id) {
                    p.children.insert(id);
                }
            }
            inner.sessions.insert(
                id,
                SessionEntry {
                    reg: reg.clone(),
                    children: HashSet::new(),
                },
            );
        }
        let _ = self.record(
            "guard.session.registered",
            json!({ "session_id": id, "parent": reg.session.parent_session_id, "mode": reg.session.mode,
                    "harness": reg.session.harness, "capabilities": reg.session.capabilities }),
        );
        Ok(())
    }

    pub fn end_session(&self, id: Uuid) -> bool {
        let removed =
            {
                let Ok(mut inner) = self.inner.lock() else {
                    return false;
                };
                // Do not orphan descendants with authority granted by a parent
                // that no longer exists. The runtime must end children first.
                if inner
                    .sessions
                    .get(&id)
                    .is_some_and(|s| !s.children.is_empty())
                {
                    return false;
                }
                if inner.approvals.values().any(|p| {
                    p.request.session_id == id && matches!(p.state, PendingState::Executing)
                }) {
                    return false;
                }
                let removed = inner.sessions.remove(&id);
                if let Some(entry) = &removed {
                    if let Some(p) = entry.reg.session.parent_session_id {
                        if let Some(parent) = inner.sessions.get_mut(&p) {
                            parent.children.remove(&id);
                        }
                    }
                    // Pending approvals of an ended session can never execute.
                    for p in inner.approvals.values_mut() {
                        if p.request.session_id == id && matches!(p.state, PendingState::Waiting) {
                            p.state = PendingState::Resolved(Box::new(SecurityDecision::new(
                                &p.request,
                                Verdict::Deny,
                                Reason::SessionUnknown,
                                Enforcement::Enforced,
                            )));
                        }
                    }
                }
                removed.is_some()
            };
        if removed {
            let _ = std::fs::remove_file(self.session_socket(id));
            let _ = std::fs::remove_file(
                self.cfg
                    .runtime_dir
                    .join("sessions")
                    .join(format!("{id}.egress.sock")),
            );
            let _ = self.record("guard.session.ended", json!({ "session_id": id }));
            self.approval_notify.notify_waiters();
        }
        removed
    }

    /// The sandbox argv for a registered session.
    ///
    /// # Errors
    /// Unknown session or invalid profile.
    pub fn launch_argv(&self, id: Uuid, cmd: &[String]) -> Result<Vec<String>, String> {
        let session = {
            let inner = self.inner.lock().map_err(|_| "state lock".to_string())?;
            inner
                .sessions
                .get(&id)
                .map(|s| s.reg.session.clone())
                .ok_or("unknown session")?
        };
        if session.mode != SessionMode::Managed {
            return Err("only managed sessions are launched by Guard".into());
        }
        let profile = SandboxProfile::for_session(&session)?;
        let host = HostPaths {
            guard_exec: self.cfg.guard_exec.clone(),
            guard_socket: Some(self.session_socket(id)),
            // Bind only the session gateway; the sandbox has no host networking.
            egress_socket: {
                let path = self
                    .cfg
                    .runtime_dir
                    .join("sessions")
                    .join(format!("{id}.egress.sock"));
                path.exists().then_some(path)
            },
        };
        let argv = profile.command_line(&host, cmd);
        let _ = self.record("guard.session.confined", json!({ "session_id": id, "mode": "managed",
            "layers": ["namespaces", "clearenv", "netns", "landlock", "seccomp"], "egress": profile.egress }));
        Ok(argv)
    }

    /// Decide a request. Brokered actions that are allowed are executed here.
    pub async fn decide(&self, req: ActionRequest) -> SecurityDecision {
        let found = self
            .inner
            .lock()
            .ok()
            .and_then(|i| i.sessions.get(&req.session_id).map(|s| s.reg.clone()));
        let Some(reg) = found else {
            let d = SecurityDecision::new(
                &req,
                Verdict::Deny,
                Reason::SessionUnknown,
                Enforcement::Enforced,
            );
            self.log_decision(&req, &d);
            return d;
        };
        let outcome = self.policy.evaluate(&reg.session, &req);
        let mut d =
            SecurityDecision::new(&req, outcome.verdict, outcome.reason, Enforcement::Enforced);
        // A decision alone does not mediate arbitrary commands in the
        // sandbox. Only the broker can enforce its own executed operation.
        if req.action.kind != CapabilityKind::GitPush {
            d.enforcement = Enforcement::Advisory;
        }
        d.policy_id = outcome.policy_id;

        match outcome.verdict {
            Verdict::Allow => {
                if req.action.kind == CapabilityKind::GitPush {
                    d = self.execute_brokered(&req, &reg, d).await;
                }
            }
            Verdict::ApprovalRequired => {
                let info = ApprovalInfo {
                    approval_id: Uuid::new_v4(),
                    digest: req.digest(),
                    summary: summary(&req),
                    risk: risk_of(req.action.kind),
                    expires_at: now_secs() + self.cfg.approval_ttl.as_secs(),
                };
                if self.record("guard.approval.requested", json!({ "approval_id": info.approval_id, "session_id": req.session_id,
                    "summary": info.summary, "risk": info.risk, "expires_at": info.expires_at, "digest": info.digest })).is_err() {
                    d.decision = Verdict::Deny;
                    d.reason = Reason::GuardUnavailable;
                    return d;
                }
                d.approval = Some(info.clone());
                if let Ok(mut inner) = self.inner.lock() {
                    inner.approvals.insert(
                        info.approval_id,
                        Pending {
                            request: req.clone(),
                            info: info.clone(),
                            state: PendingState::Waiting,
                        },
                    );
                } else {
                    d.approval = None;
                    d.decision = Verdict::Deny;
                    d.reason = Reason::GuardUnavailable;
                }
            }
            Verdict::Deny => {}
        }
        self.log_decision(&req, &d);
        d
    }

    fn log_decision(&self, req: &ActionRequest, d: &SecurityDecision) {
        let data = json!({ "request_id": req.request_id, "session_id": req.session_id, "kind": req.action.kind,
            "target": req.action.target, "decision": d.decision, "reason": d.reason, "policy_id": d.policy_id });
        if self.record("guard.decision", data).is_err() {
            eprintln!(
                "cerynth-guardd: AUDIT WRITE FAILED for request {}",
                req.request_id
            );
        }
    }

    async fn execute_brokered(
        &self,
        req: &ActionRequest,
        reg: &Registration,
        mut d: SecurityDecision,
    ) -> SecurityDecision {
        let fail = |mut d: SecurityDecision, msg: String| {
            d.decision = Verdict::Deny;
            d.reason = Reason::BrokerFailed;
            d.result = Some(json!({ "error": msg }));
            d
        };
        let spec = match git::parse(&req.action.target, &req.action.args) {
            Ok(s) => s,
            Err(e) => return fail(d, e),
        };
        let Some(url) = reg.remotes.get(&spec.remote) else {
            return fail(
                d,
                format!("remote {} is not registered for this session", spec.remote),
            );
        };
        // Do not perform an external side effect if its intent cannot first
        // be durably recorded. A later completion record can still fail.
        if self
            .record(
                "guard.broker.git_push.started",
                json!({
                    "request_id": req.request_id, "session_id": req.session_id,
                    "target": req.action.target, "digest": req.digest()
                }),
            )
            .is_err()
        {
            return fail(d, "audit unavailable".into());
        }
        // A git credential named like the remote, if the session may use it.
        let cred_name = format!("git-{}", spec.remote);
        let credential = reg
            .session
            .capabilities
            .grant(CapabilityKind::CredentialUse, &cred_name)
            // A git push approval is not an approval to use a separate
            // credential capability. Until combined approvals are supported,
            // only an explicit allow can release the credential.
            .filter(|m| *m == Mode::Allow)
            .and_then(|_| self.vault.get(&cred_name));
        match git::push(
            &spec,
            url,
            &reg.session.workspace,
            &self.cfg.mirror_dir,
            &req.session_id.to_string(),
            credential,
            &self.vault,
            git::UrlPolicy {
                allow_file: self.cfg.allow_file_remotes,
            },
        )
        .await
        {
            Ok(result) => {
                d.result = Some(result);
                d
            }
            Err(e) => fail(d, e),
        }
    }

    /// Resolve an approval from the desktop. Single-use, digest-bound, expiring.
    pub async fn approve(
        &self,
        resp: ApprovalResponse,
    ) -> Result<SecurityDecision, (Reason, String)> {
        let (req, info) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| (Reason::GuardUnavailable, "state lock".into()))?;
            let p = inner
                .approvals
                .get_mut(&resp.approval_id)
                .ok_or((Reason::ApprovalUnknown, "no such approval".into()))?;
            if !matches!(p.state, PendingState::Waiting) {
                return Err((
                    Reason::ApprovalUnknown,
                    "approval already claimed or resolved".into(),
                ));
            }
            if now_secs() > p.info.expires_at {
                let d = SecurityDecision::new(
                    &p.request,
                    Verdict::Deny,
                    Reason::ApprovalExpired,
                    Enforcement::Enforced,
                );
                p.state = PendingState::Resolved(Box::new(d));
                drop(inner);
                self.approval_notify.notify_waiters();
                return Err((Reason::ApprovalExpired, "approval expired".into()));
            }
            if resp.digest != p.info.digest || p.request.digest() != p.info.digest {
                drop(inner);
                let _ = self.record(
                    "guard.approval.mismatch",
                    json!({ "approval_id": resp.approval_id }),
                );
                return Err((
                    Reason::ApprovalMismatch,
                    "digest does not match the pending operation".into(),
                ));
            }
            if self.record("guard.approval.claimed", json!({"approval_id": resp.approval_id, "session_id": p.request.session_id, "digest": p.info.digest})).is_err() {
                return Err((Reason::GuardUnavailable, "cannot record approval claim".into()));
            }
            // Claim under the lock before awaiting execution. Concurrent
            // approvers must never execute the same external operation twice.
            p.state = PendingState::Executing;
            (p.request.clone(), p.info.clone())
        };

        let mut d = match resp.response {
            ApprovalAnswer::Deny => SecurityDecision::new(
                &req,
                Verdict::Deny,
                Reason::ApprovalDenied,
                Enforcement::Enforced,
            ),
            ApprovalAnswer::Approve => {
                let reg = self
                    .inner
                    .lock()
                    .ok()
                    .and_then(|i| i.sessions.get(&req.session_id).map(|s| s.reg.clone()));
                match reg {
                    None => SecurityDecision::new(
                        &req,
                        Verdict::Deny,
                        Reason::SessionUnknown,
                        Enforcement::Enforced,
                    ),
                    Some(reg) => {
                        let d = SecurityDecision::new(
                            &req,
                            Verdict::Allow,
                            Reason::PolicyAllow,
                            Enforcement::Enforced,
                        );
                        if req.action.kind == CapabilityKind::GitPush {
                            self.execute_brokered(&req, &reg, d).await
                        } else {
                            d
                        }
                    }
                }
            }
        };
        d.approval = Some(info.clone());
        if req.action.kind != CapabilityKind::GitPush {
            d.enforcement = Enforcement::Advisory;
        }
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(p) = inner.approvals.get_mut(&resp.approval_id) {
                p.state = PendingState::Resolved(Box::new(d.clone()));
            }
        }
        let _ = self.record("guard.approval.resolved", json!({ "approval_id": info.approval_id, "session_id": req.session_id,
            "response": resp.response, "responder": resp.responder, "executed": d.result.is_some(), "decision": d.decision, "reason": d.reason }));
        self.approval_notify.notify_waiters();
        Ok(d)
    }

    /// Wait until an approval of `session` is resolved (or expires).
    pub async fn await_approval(
        &self,
        approval_id: Uuid,
        session: Option<Uuid>,
    ) -> Result<SecurityDecision, (Reason, String)> {
        loop {
            let notified = self.approval_notify.notified();
            {
                let mut inner = self
                    .inner
                    .lock()
                    .map_err(|_| (Reason::GuardUnavailable, "state lock".into()))?;
                let p = inner
                    .approvals
                    .get_mut(&approval_id)
                    .ok_or((Reason::ApprovalUnknown, "no such approval".into()))?;
                if session.is_some_and(|s| s != p.request.session_id) {
                    return Err((Reason::ApprovalUnknown, "no such approval".into()));
                }
                match &p.state {
                    PendingState::Resolved(d) => return Ok((**d).clone()),
                    PendingState::Waiting if now_secs() > p.info.expires_at => {
                        let d = SecurityDecision::new(
                            &p.request,
                            Verdict::Deny,
                            Reason::ApprovalExpired,
                            Enforcement::Enforced,
                        );
                        p.state = PendingState::Resolved(Box::new(d.clone()));
                        return Ok(d);
                    }
                    PendingState::Waiting | PendingState::Executing => {}
                }
            }
            let _ = tokio::time::timeout(Duration::from_secs(1), notified).await;
        }
    }

    pub fn pending(&self) -> Vec<PendingView> {
        let Ok(inner) = self.inner.lock() else {
            return vec![];
        };
        let now = now_secs();
        inner
            .approvals
            .values()
            .filter(|p| matches!(p.state, PendingState::Waiting) && p.info.expires_at >= now)
            .map(|p| PendingView {
                session_id: p.request.session_id,
                approval: p.info.clone(),
                action: p.request.action.clone(),
            })
            .collect()
    }

    pub fn counts(&self) -> (usize, usize) {
        self.inner.lock().map_or((0, 0), |i| {
            (
                i.sessions.len(),
                i.approvals
                    .values()
                    .filter(|p| matches!(p.state, PendingState::Waiting))
                    .count(),
            )
        })
    }

    pub fn audit_path(&self) -> PathBuf {
        self.audit
            .lock()
            .map(|a| a.path().to_path_buf())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod security_regressions {
    use super::*;
    use cerynth_security::{Capability, CapabilitySet, GuardSession};
    use std::collections::BTreeMap;

    fn setup() -> (tempfile::TempDir, Arc<Guard>) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::GuardConfig {
            runtime_dir: dir.path().join("run"),
            state_dir: dir.path().join("state"),
            ..Default::default()
        };
        let guard = cfg.build().unwrap();
        (dir, guard)
    }

    fn registration(
        dir: &std::path::Path,
        id: Uuid,
        parent: Option<Uuid>,
        caps: Vec<Capability>,
    ) -> Registration {
        Registration {
            session: GuardSession {
                session_id: id,
                parent_session_id: parent,
                mode: SessionMode::Managed,
                workspace: dir.to_path_buf(),
                capabilities: CapabilitySet(caps),
                harness: None,
            },
            remotes: BTreeMap::new(),
        }
    }

    #[test]
    fn delegation_requiring_approval_is_not_silently_allowed() {
        let (dir, guard) = setup();
        let parent = Uuid::new_v4();
        let cap = Capability {
            limit: Some(1),
            ..Capability::new(CapabilityKind::SessionSpawn, "child", Mode::Approval)
        };
        guard
            .register(registration(dir.path(), parent, None, vec![cap]))
            .unwrap();
        assert!(
            guard
                .register(registration(
                    dir.path(),
                    Uuid::new_v4(),
                    Some(parent),
                    vec![]
                ))
                .is_err()
        );
    }

    #[test]
    fn parent_cannot_end_before_child() {
        let (dir, guard) = setup();
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        let cap = Capability {
            limit: Some(1),
            ..Capability::new(CapabilityKind::SessionSpawn, "child", Mode::Allow)
        };
        guard
            .register(registration(dir.path(), parent, None, vec![cap]))
            .unwrap();
        guard
            .register(registration(dir.path(), child, Some(parent), vec![]))
            .unwrap();
        assert!(!guard.end_session(parent));
        assert!(guard.end_session(child));
        assert!(guard.end_session(parent));
    }

    #[tokio::test]
    async fn claimed_approval_cannot_be_replayed() {
        let (_dir, guard) = setup();
        let req = ActionRequest::new(
            Uuid::new_v4(),
            CapabilityKind::GitPush,
            "origin:refs/heads/a",
            json!({}),
        );
        let info = ApprovalInfo {
            approval_id: Uuid::new_v4(),
            digest: req.digest(),
            summary: String::new(),
            risk: Risk::High,
            expires_at: now_secs() + 60,
        };
        guard.inner.lock().unwrap().approvals.insert(
            info.approval_id,
            Pending {
                request: req,
                info: info.clone(),
                state: PendingState::Executing,
            },
        );
        let response = ApprovalResponse {
            schema: cerynth_security::decision::SCHEMA.into(),
            approval_id: info.approval_id,
            digest: info.digest,
            response: ApprovalAnswer::Approve,
            responder: None,
        };
        assert!(guard.approve(response).await.is_err());
    }

    #[test]
    fn malformed_unicode_commit_does_not_panic() {
        let req = ActionRequest::new(
            Uuid::new_v4(),
            CapabilityKind::GitPush,
            "origin:x",
            json!({"commit": "ééééééééééééé"}),
        );
        assert!(summary(&req).contains('é'));
    }

    #[test]
    fn restart_invalidates_pending_and_marks_claimed_indeterminate() {
        let (dir, guard) = setup();
        let pending = Uuid::new_v4();
        let claimed = Uuid::new_v4();
        guard
            .record("guard.approval.requested", json!({"approval_id": pending}))
            .unwrap();
        guard
            .record("guard.approval.requested", json!({"approval_id": claimed}))
            .unwrap();
        guard
            .record("guard.approval.claimed", json!({"approval_id": claimed}))
            .unwrap();
        drop(guard);
        let cfg = crate::config::GuardConfig {
            runtime_dir: dir.path().join("run"),
            state_dir: dir.path().join("state"),
            ..Default::default()
        };
        let restarted = cfg.build().unwrap();
        let text = std::fs::read_to_string(restarted.audit_path()).unwrap();
        assert!(
            text.lines()
                .any(|line| line.contains(&pending.to_string()) && line.contains("cancelled"))
        );
        assert!(text.lines().any(|line| line.contains(&claimed.to_string())
            && line.contains("indeterminate_do_not_retry_automatically")));
        assert_eq!(restarted.pending().len(), 0);
    }
}
