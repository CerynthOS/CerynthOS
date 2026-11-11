//! Experimental A2A v0.3 JSON-RPC task subset over the owner control API.
//! This is not an HTTP A2A server or a claim of full protocol conformance.
use crate::AgentService;
use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{Session, SessionState, TaskAssignment};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

impl AgentService {
    // Three wire operations share one sponsor-scoped authorization transaction.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn a2a(&self, sponsor: Uuid, request: Value) -> Result<Value> {
        ensure!(
            request["jsonrpc"] == "2.0" && request.get("id").is_some(),
            "invalid JSON-RPC envelope"
        );
        let result = match request["method"].as_str().context("missing A2A method")? {
            "message/send" => {
                let message = &request["params"]["message"];
                ensure!(
                    message["role"] == "user",
                    "only user task assignments are supported"
                );
                let message_id = message["messageId"]
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 256)
                    .context("missing or invalid A2A messageId")?;
                let parts = message["parts"].as_array().context("missing task parts")?;
                ensure!(
                    !parts.is_empty()
                        && parts.len() <= 16
                        && parts
                            .iter()
                            .all(|p| p["kind"] == "text" && p["text"].is_string()),
                    "only text task parts are supported; files/data require separate authorization"
                );
                let text = parts
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                ensure!(
                    !text.trim().is_empty() && text.len() <= 8192,
                    "invalid delegated task text"
                );
                // Lower-trust protocol metadata never specifies capabilities, budgets,
                // credentials, workspace paths, or cloud routing.
                ensure!(
                    message.get("metadata").is_none()
                        && request["params"].get("metadata").is_none(),
                    "metadata cannot grant delegation authority"
                );
                let digest = Sha256::digest(format!("{sponsor}:{message_id}").as_bytes());
                let mut bytes = [0; 16];
                bytes.copy_from_slice(&digest[..16]);
                let task_id = Uuid::from_bytes(bytes);
                let mut runtime = self.runtime.lock().await;
                let parent = runtime.registry.get(sponsor)?;
                ensure!(
                    parent.spec.runtime.capabilities.contains("task.delegate"),
                    "delegation capability is not granted"
                );
                if let Some(existing) = runtime.registry.list().into_iter().find(|s| {
                    s.spec.runtime.parent_id == Some(sponsor)
                        && s.spec
                            .runtime
                            .task
                            .as_ref()
                            .is_some_and(|task| task.id == task_id)
                }) {
                    ensure!(
                        existing
                            .spec
                            .runtime
                            .task
                            .as_ref()
                            .is_some_and(|task| task.objective == text),
                        "A2A messageId reused with different action details"
                    );
                    task(&existing)
                } else {
                    let mut spec = parent.spec.clone();
                    spec.name = format!("a2a-child-{task_id}");
                    spec.runtime.parent_id = Some(sponsor);
                    spec.runtime.capabilities.clear();
                    spec.runtime.message_peers.clear();
                    spec.runtime.network = cerynth_agent_api::NetworkPolicy::Deny;
                    let divisor = u64::from(parent.spec.runtime.budget.max_subagents.clamp(1, 2));
                    spec.runtime.budget.memory_max_bytes /= divisor;
                    spec.runtime.budget.memory_high_bytes = spec
                        .runtime
                        .budget
                        .memory_high_bytes
                        .min(spec.runtime.budget.memory_max_bytes);
                    spec.runtime.budget.max_subagents = 0;
                    spec.runtime.budget.inference_concurrency =
                        u32::from(parent.spec.runtime.budget.inference_concurrency > 0);
                    spec.runtime.task = Some(TaskAssignment {
                        id: task_id,
                        objective: text,
                        completion_criteria:
                            "Owner must inspect produced artifacts and test evidence".into(),
                        expected_output: "task_result artifact".into(),
                        dependencies: std::collections::BTreeSet::default(),
                    });
                    task(&runtime.registry.create(spec)?)
                }
            }
            "tasks/get" | "tasks/cancel" => {
                let id: Uuid = request["params"]["id"]
                    .as_str()
                    .context("missing task ID")?
                    .parse()?;
                let session = self.runtime.lock().await.registry.get(id)?;
                ensure!(
                    session.spec.runtime.parent_id == Some(sponsor),
                    "task is outside the sponsor scope"
                );
                if request["method"] == "tasks/cancel" {
                    self.stop_session(id).await?;
                    task(&self.runtime.lock().await.registry.get(id)?)
                } else {
                    task(&session)
                }
            }
            _ => anyhow::bail!(
                "unsupported A2A method; subset: message/send, tasks/get, tasks/cancel"
            ),
        };
        Ok(json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
    }
}

fn task(session: &Session) -> Value {
    let state = match session.state {
        SessionState::Created => "submitted",
        SessionState::Completed => "completed",
        SessionState::Failed => "failed",
        SessionState::Cancelled => "canceled",
        SessionState::WaitingForApproval => "auth-required",
        SessionState::WaitingForTool => "input-required",
        _ => "working",
    };
    json!({"kind":"task","id":session.id,"contextId":session.spec.runtime.project,
        "status":{"state":state},"artifacts":session.execution.artifacts,
        "metadata":{"cerynth_task_completion_verified":false,"automatic_execution":false}})
}
