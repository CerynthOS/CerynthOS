//! Wire protocol of `/run/cerynth/guard.sock` and the per-session sockets.
//! Newline-delimited JSON, one request per line, one response per line
//! (`Subscribe` keeps streaming events).

use std::collections::BTreeMap;

use cerynth_security::{
    ActionRequest, ApprovalInfo, ApprovalResponse, GuardSession, SecurityDecision,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Registration from the runtime (P3's agentd). The agent never sends this.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registration {
    #[serde(flatten)]
    pub session: GuardSession,
    /// Git remotes Guard may push to for this session: name → URL. Set by the
    /// runtime/user, never read from the agent's repository config.
    #[serde(default)]
    pub remotes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    RegisterSession {
        registration: Registration,
    },
    EndSession {
        session_id: Uuid,
    },
    /// Ask for a decision. On the per-session socket `session_id` is forced
    /// to that socket's session.
    Decide {
        request: ActionRequest,
    },
    /// Block until an approval is resolved; returns the final decision.
    AwaitApproval {
        approval_id: Uuid,
    },
    Approve {
        response: ApprovalResponse,
    },
    PendingApprovals,
    /// The sandbox command line for a registered session.
    LaunchProfile {
        session_id: Uuid,
        cmd: Vec<String>,
    },
    Subscribe,
    VerifyAudit,
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Decision {
        decision: SecurityDecision,
    },
    Pending {
        approvals: Vec<PendingView>,
    },
    Launch {
        argv: Vec<String>,
    },
    Audit {
        records: u64,
        last_hash: String,
    },
    Status {
        sessions: usize,
        pending_approvals: usize,
        policy: String,
        audit: String,
    },
    Event {
        event: serde_json::Value,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingView {
    pub session_id: Uuid,
    pub approval: ApprovalInfo,
    pub action: cerynth_security::Action,
}

impl Response {
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }
}
