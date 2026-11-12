use crate::{AgentService, process::PreparedLaunch};
use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{HarnessKind, Session, SessionMode};
use cerynth_inferenced::{AdminOperation, SessionGrant};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};
use uuid::Uuid;

pub(crate) struct InferenceControl {
    directory: PathBuf,
    http_origin: String,
}

impl InferenceControl {
    pub(crate) fn directory(&self) -> &std::path::Path {
        &self.directory
    }
    pub fn from_environment() -> Result<Option<Self>> {
        match (
            std::env::var_os("CERYNTH_INFERENCE_DIR"),
            std::env::var("CERYNTH_INFERENCE_HTTP").ok(),
        ) {
            (None, None) => Ok(None),
            (Some(directory), Some(http_origin)) => {
                cerynth_model_api::OllamaClient::new(&http_origin)
                    .context("invalid inference proxy origin")?;
                let directory = PathBuf::from(directory);
                ensure!(
                    directory.is_absolute(),
                    "inference control directory must be absolute"
                );
                Ok(Some(Self {
                    directory,
                    http_origin,
                }))
            }
            _ => anyhow::bail!(
                "set both CERYNTH_INFERENCE_DIR and CERYNTH_INFERENCE_HTTP for brokered harness inference"
            ),
        }
    }
}

impl AgentService {
    pub(crate) fn pressure_sampler(
        self: &std::sync::Arc<Self>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        if self.resources.is_none() || self.inference_control.is_none() {
            return None;
        }
        let weak = std::sync::Arc::downgrade(self);
        Some(tokio::spawn(async move {
            loop {
                let Some(service) = weak.upgrade() else {
                    return;
                };
                let Some(control) = &service.inference_control else {
                    return;
                };
                let sample = cerynth_resources::pressure::sample();
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    cerynth_inferenced::call(
                        &control.directory,
                        true,
                        serde_json::json!({"type":"pressure","sample":sample}),
                    ),
                )
                .await;
                drop(service);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }))
    }
    async fn inference_scopes(&self, session: &Session) -> Result<Vec<(String, usize)>> {
        let runtime = self.runtime.lock().await;
        let project_limit = std::env::var("CERYNTH_PROJECT_INFERENCE_LIMIT")
            .unwrap_or_else(|_| "4".into())
            .parse::<usize>()
            .context("invalid project inference limit")?;
        ensure!(
            (1..=32).contains(&project_limit),
            "project inference limit must be 1..32"
        );
        let mut scopes = vec![
            (
                format!(
                    "project:{}:{}",
                    session.owner_uid, session.spec.runtime.project
                ),
                project_limit,
            ),
            (
                format!("session:{}", session.id),
                session.spec.runtime.budget.inference_concurrency as usize,
            ),
        ];
        let mut parent = session.spec.runtime.parent_id;
        while let Some(id) = parent {
            ensure!(
                scopes.len() < 32,
                "session ancestry exceeds inference scope limit"
            );
            let ancestor = runtime.registry.get(id)?;
            scopes.push((
                format!("session:{id}"),
                ancestor.spec.runtime.budget.inference_concurrency as usize,
            ));
            parent = ancestor.spec.runtime.parent_id;
        }
        Ok(scopes)
    }

    pub(crate) async fn bind_inference(
        &self,
        session: &Session,
        plan: &mut PreparedLaunch,
    ) -> Result<()> {
        let Some(endpoint_id) = &session.spec.model.endpoint_id else {
            ensure!(
                session.spec.model.mode == cerynth_agent_api::ModelMode::Local,
                "cloud model sessions require an explicitly configured coordinator endpoint"
            );
            return Ok(());
        };
        let control = self.inference_control.as_ref().context(
            "session requires inference coordinator; daemon connection is not configured",
        )?;
        ensure!(
            matches!(
                session.spec.harness,
                HarnessKind::OpenCode | HarnessKind::Codex | HarnessKind::Hermes
            ),
            "this harness does not support the configured inference proxy transport"
        );
        let scopes = self.inference_scopes(session).await?;
        let grant = SessionGrant {
            session_id: session.id,
            endpoint_id: endpoint_id.clone(),
            trust_domain: format!("owner:{}", session.owner_uid),
            allowed_models: BTreeSet::from([session.spec.model.name.clone()]),
            concurrency_limits: scopes,
            max_priority: 0,
            disable_thinking: session.spec.model.disable_thinking,
            allow_cloud: session.spec.model.mode == cerynth_agent_api::ModelMode::Cloud
                && session.spec.model.cloud_consent,
        };
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            cerynth_inferenced::call(
                &control.directory,
                true,
                serde_json::to_value(AdminOperation::Register { grant })?,
            ),
        )
        .await
        .context("inference coordinator registration timed out")??;
        let token = response["token"]
            .as_str()
            .context("coordinator returned no session credential")?;
        if session
            .spec
            .runtime
            .guard
            .as_ref()
            .is_some_and(|guard| guard.native_inference)
        {
            return self
                .bind_native_guard(session, token, &control.http_origin, plan)
                .await;
        }
        let url = format!("{}/v1", control.http_origin.trim_end_matches('/'));
        match session.spec.harness {
            HarnessKind::OpenCode => {
                let value = plan
                    .environment
                    .iter_mut()
                    .find(|(key, _)| key == "OPENCODE_CONFIG_CONTENT")
                    .context("OpenCode local provider configuration is missing")?;
                let mut config: serde_json::Value = serde_json::from_str(&value.1)?;
                config["provider"]["ollama"]["options"]["baseURL"] = serde_json::json!(url);
                config["provider"]["ollama"]["name"] =
                    serde_json::json!("Cerynth configured inference");
                config["provider"]["ollama"]["options"]["apiKey"] = serde_json::json!(token);
                value.1 = config.to_string();
            }
            HarnessKind::Codex => {
                plan.args = replace_codex_args(&plan.args, &url)?;
                plan.environment
                    .push(("CERYNTH_INFERENCE_TOKEN".into(), token.into()));
            }
            HarnessKind::Hermes => {
                let position = plan
                    .args
                    .iter()
                    .position(|arg| arg == "--base-url")
                    .context("Hermes endpoint flag missing")?;
                plan.args
                    .get_mut(position + 1)
                    .context("Hermes endpoint value missing")?
                    .clone_from(&url);
                plan.environment.retain(|(key, _)| key != "OPENAI_API_KEY");
                plan.environment.push(("OPENAI_BASE_URL".into(), url));
                plan.environment
                    .push(("OPENAI_API_KEY".into(), token.into()));
            }
            _ => unreachable!("supported harness checked above"),
        }
        Ok(())
    }

    pub(crate) async fn revoke_inference(&self, id: Uuid) {
        if let Some(control) = &self.inference_control {
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                cerynth_inferenced::call(
                    &control.directory,
                    true,
                    serde_json::json!({"type":"revoke","session_id":id}),
                ),
            )
            .await;
        }
    }

    pub(crate) async fn recover_inference_grants(&self) {
        let sessions = self.runtime.lock().await.registry.list();
        for session in sessions {
            if session.state.is_terminal()
                && session.execution.mode != SessionMode::Observed
                && session.spec.model.endpoint_id.is_some()
                && !session.execution.operations.is_empty()
            {
                self.revoke_inference(session.id).await;
            }
        }
    }
}

fn replace_codex_args(args: &[String], url: &str) -> Result<Vec<String>> {
    let mut prefix = if let Some(position) = args.iter().rposition(|arg| arg == "--") {
        let executable = args
            .get(position + 1)
            .context("prepared executable missing")?
            .clone();
        let mut prefix = args[..=position].to_vec();
        prefix.push(executable);
        prefix
    } else {
        Vec::new()
    };
    let model = args
        .iter()
        .position(|arg| arg == "--model")
        .and_then(|i| args.get(i + 1))
        .context("Codex model missing")?;
    prefix.extend([
        "--no-daemon".into(),
        "-c".into(),
        "model_provider=\"cerynth\"".into(),
        "-c".into(),
        "model_providers.cerynth.name=\"Cerynth\"".into(),
        "-c".into(),
        format!(
            "model_providers.cerynth.base_url={}",
            serde_json::to_string(url)?
        ),
        "-c".into(),
        "model_providers.cerynth.env_key=\"CERYNTH_INFERENCE_TOKEN\"".into(),
        "-c".into(),
        "model_providers.cerynth.wire_api=\"responses\"".into(),
        "exec".into(),
        "--json".into(),
        "--ephemeral".into(),
        "--ignore-user-config".into(),
        "--ignore-rules".into(),
        "--sandbox".into(),
        "workspace-write".into(),
        "--model".into(),
        model.clone(),
        "-".into(),
    ]);
    Ok(prefix)
}

#[cfg(test)]
mod control_boundary_tests {
    use super::*;

    #[test]
    fn generic_workspaces_cannot_mount_separately_configured_inference_control() -> Result<()> {
        let root = tempfile::tempdir()?;
        let workspace = root.path().join("workspace");
        let separate = root.path().join("allowed");
        std::fs::create_dir(&workspace)?;
        std::fs::create_dir(&separate)?;
        let directory = workspace.join("inference-control");
        crate::registry::private_directory(&directory)?;
        let mut service = AgentService::open(&root.path().join("agent"), "http://127.0.0.1:11434")?;
        service.inference_control = Some(InferenceControl {
            directory,
            http_origin: "http://127.0.0.1:11435".into(),
        });
        assert!(service.check_control_workspace(&workspace).is_err());
        assert!(service.check_control_workspace(&separate).is_ok());
        Ok(())
    }
}
