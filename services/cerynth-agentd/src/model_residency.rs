//! Owner-authorized model release tied to the exact supervised GPU session.
use crate::{AgentService, registry::Registry};
use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{HarnessKind, ModelResidencyBinding, SessionState};
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

impl AgentService {
    pub(crate) async fn quiesce_gpu_model(
        &self,
        id: Uuid,
        lease_id: Uuid,
        endpoint_id: String,
        model: String,
    ) -> Result<Value> {
        let control = self
            .inference_control
            .as_ref()
            .context("inference coordinator is not configured")?;
        {
            let mut runtime = self.runtime.lock().await;
            let source = runtime.registry.get(id)?;
            ensure!(
                source.state == SessionState::Created
                    && source.spec.harness == HarnessKind::GenericPty
                    && source.spec.runtime.guard.is_some()
                    && source.spec.runtime.capabilities.contains("gpu.execute")
                    && source.spec.runtime.capabilities.contains("model.release")
                    && source.execution.gpu_job_id.is_none(),
                "model release requires a new Guard GPU session and explicit model.release authority"
            );
            if let Some(binding) = &source.execution.model_residency {
                ensure!(
                    binding.lease_id == lease_id
                        && binding.endpoint_id == endpoint_id
                        && binding.model == model,
                    "model residency operation details changed"
                );
            } else {
                runtime.registry.update(id, |s| {
                    s.execution.model_residency = Some(ModelResidencyBinding {
                        lease_id,
                        endpoint_id: endpoint_id.clone(),
                        model: model.clone(),
                        ready: false,
                    });
                    s.execution.model_residency_cleanup_pending = true;
                    Registry::record_event(s, "model_residency_preparing");
                    Ok(())
                })?;
            }
        }
        let result=tokio::time::timeout(Duration::from_secs(20),cerynth_inferenced::call(control.directory(),true,
            json!({"type":"quiesce_model","lease_id":lease_id,"session_id":id,"endpoint_id":endpoint_id,"model":model}))).await;
        let result = result
            .context("model residency preparation timed out")
            .and_then(|r| r);
        match result {
            Ok(receipt) => {
                ensure!(
                    receipt["ready"] == true && receipt["lease_id"] == lease_id.to_string(),
                    "model residency is not ready"
                );
                self.runtime.lock().await.registry.update(id, |s| {
                    ensure!(
                        s.state == SessionState::Created
                            && s.execution.model_residency_cleanup_pending,
                        "GPU model request was cancelled"
                    );
                    s.execution
                        .model_residency
                        .as_mut()
                        .context("model binding disappeared")?
                        .ready = true;
                    Registry::record_event(s, "model_residency_ready");
                    Ok(())
                })?;
                Ok(receipt)
            }
            Err(error) => {
                let mut runtime = self.runtime.lock().await;
                if !runtime.registry.get(id)?.state.is_terminal() {
                    runtime.registry.transition(
                        id,
                        SessionState::Failed,
                        "model_residency_prepare_failed",
                    )?;
                }
                drop(runtime);
                let _ = self.release_model_residency().await;
                Err(error)
            }
        }
    }

    pub(crate) async fn release_model_residency(&self) -> Result<()> {
        let Some(control) = &self.inference_control else {
            return Ok(());
        };
        let sessions = self.runtime.lock().await.registry.list();
        for source in sessions.into_iter().filter(|s| {
            s.state.is_terminal()
                && !s.execution.cleanup_pending
                && !s.execution.gpu_cleanup_pending
                && s.execution.model_residency_cleanup_pending
        }) {
            let binding = source
                .execution
                .model_residency
                .context("model lease journal missing")?;
            let receipt=tokio::time::timeout(Duration::from_secs(8),cerynth_inferenced::call(control.directory(),true,
                json!({"type":"release_model","lease_id":binding.lease_id,"session_id":source.id}))).await??;
            ensure!(
                receipt["released"] == true,
                "coordinator did not release the model gate"
            );
            self.runtime.lock().await.registry.update(source.id, |s| {
                s.execution.model_residency_cleanup_pending = false;
                Registry::record_event(s, "model_residency_released");
                Ok(())
            })?;
        }
        Ok(())
    }
}
