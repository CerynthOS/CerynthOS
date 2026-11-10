//! Transport-independent request handling and caller authorization.
//!
//! The transport (see `docs/security/HANDOFF.md`, "IPC transport") decides
//! *who* the caller is from OS-verified identity and passes a [`Caller`]:
//!
//! - [`Caller::Trusted`]: the runtime or the desktop on the main socket.
//! - [`Caller::Session`]: a sandboxed agent on its own session socket.
//!
//! Everything about *what* each caller may do is enforced here, so it is
//! covered by unit tests regardless of the transport.

use std::sync::Arc;

use cerynth_security::{Reason, SecurityDecision, Verdict};
use uuid::Uuid;

use crate::protocol::{Request, Response};
use crate::state::Guard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Trusted,
    Session(Uuid),
}

fn code(r: Reason) -> String {
    serde_json::to_value(r)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "error".into())
}

fn denied(r: Reason, msg: impl Into<String>) -> Response {
    Response::error(&code(r), msg)
}

/// Handle one request from `caller`.
pub async fn dispatch(guard: &Arc<Guard>, caller: Caller, req: Request) -> Response {
    if let Caller::Session(own) = caller {
        return match req {
            Request::Decide { mut request } => {
                if request.session_id != own {
                    // An agent naming another session is impersonation: deny,
                    // and decide nothing on the other session's behalf.
                    let mut d = SecurityDecision::new(
                        &request,
                        Verdict::Deny,
                        Reason::CallerNotAuthorized,
                        cerynth_security::Enforcement::Enforced,
                    );
                    d.session_id = own;
                    request.session_id = own;
                    return Response::Decision { decision: d };
                }
                Response::Decision {
                    decision: guard.decide(request).await,
                }
            }
            Request::AwaitApproval { approval_id } => {
                match guard.await_approval(approval_id, Some(own)).await {
                    Ok(decision) => Response::Decision { decision },
                    Err((r, m)) => denied(r, m),
                }
            }
            _ => denied(
                Reason::CallerNotAuthorized,
                "sessions may only request decisions and wait for approvals",
            ),
        };
    }

    match req {
        Request::RegisterSession { registration } => match guard.register(registration) {
            Ok(()) => Response::Ok,
            Err((r, m)) => denied(r, m),
        },
        Request::EndSession { session_id } => {
            if guard.end_session(session_id) {
                Response::Ok
            } else {
                denied(Reason::SessionUnknown, "no such session")
            }
        }
        Request::Decide { request } => Response::Decision {
            decision: guard.decide(request).await,
        },
        Request::AwaitApproval { approval_id } => {
            match guard.await_approval(approval_id, None).await {
                Ok(decision) => Response::Decision { decision },
                Err((r, m)) => denied(r, m),
            }
        }
        Request::Approve { response } => match guard.approve(response).await {
            Ok(decision) => Response::Decision { decision },
            Err((r, m)) => denied(r, m),
        },
        Request::PendingApprovals => Response::Pending {
            approvals: guard.pending(),
        },
        Request::LaunchProfile { session_id, cmd } => match guard.launch_argv(session_id, &cmd) {
            Ok(argv) => Response::Launch { argv },
            Err(m) => denied(Reason::PolicyDeny, m),
        },
        Request::VerifyAudit => match cerynth_security::audit::verify(guard.audit_path()) {
            Ok(v) => Response::Audit {
                records: v.records,
                last_hash: v.last_hash,
            },
            Err(e) => Response::error("audit_broken", e.to_string()),
        },
        Request::Status => {
            let (sessions, pending_approvals) = guard.counts();
            Response::Status {
                sessions,
                pending_approvals,
                policy: guard.cfg.policy_source.clone(),
                audit: guard.audit_path().display().to_string(),
            }
        }
        Request::Subscribe => Response::error(
            "unsupported",
            "event streaming is provided by the transport (guard.events channel)",
        ),
    }
}
