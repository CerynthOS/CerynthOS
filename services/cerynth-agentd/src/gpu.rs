//! Owned GPU execution reuses process supervision and Person 2 authorization.
use crate::{
    AgentService, process,
    registry::{Registry, now_ms},
};
use anyhow::{Result, ensure};
use cerynth_agent_api::{HarnessKind, LaunchRequest, SessionState};
use cerynth_resources::JobState;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

impl AgentService {
    pub(crate) async fn run_gpu(
        self: &Arc<Self>,
        id: Uuid,
        job_id: Uuid,
        operation_id: Uuid,
        launch: LaunchRequest,
    ) -> Result<Value> {
        // A duplicate launch goes through the durable operation ledger. It must
        // not rebind or consume a second reservation.
        {
            let mut runtime = self.runtime.lock().await;
            let session = runtime.registry.get(id)?;
            if session.execution.gpu_job_id == Some(job_id)
                && session
                    .execution
                    .operations
                    .iter()
                    .any(|op| op.id == operation_id)
            {
                drop(runtime);
                return self.start_generic(id, operation_id, launch).await;
            }
            ensure!(
                session.state == SessionState::Created
                    && session.spec.harness == HarnessKind::GenericPty
                    && session.spec.runtime.guard.is_some()
                    && session.spec.runtime.capabilities.contains("gpu.execute")
                    && session.execution.gpu_job_id.is_none(),
                "GPU execution requires a new generic session with Guard and gpu.execute"
            );
            process::prepare(&session, &launch, false)?;
            ensure!(
                session
                    .spec
                    .runtime
                    .guard
                    .as_ref()
                    .is_some_and(|policy| policy
                        .capabilities
                        .iter()
                        .filter(|cap| cap.kind == "gpu.use")
                        .all(|cap| cap.target == "*")),
                "this Guard GPU profile grants all device nodes; scoped device partitions are unsupported"
            );
            let mut gpu = self.gpu.lock().await;
            let job = gpu
                .jobs
                .get(&job_id)
                .ok_or_else(|| anyhow::anyhow!("GPU lease unavailable"))?;
            ensure!(
                job.request.session_id == id
                    && job.request.memory_bytes <= session.spec.runtime.budget.gpu_memory_bytes,
                "GPU lease owner or budget mismatch"
            );
            gpu.bind_process(job_id, now_ms())?;
            if let Err(error) = runtime.registry.update(id, |s| {
                s.execution.gpu_job_id = Some(job_id);
                s.execution.gpu_cleanup_pending = true;
                s.execution.cleanup_pending = true;
                Registry::record_event(s, "gpu_execution_bound");
                Ok(())
            }) {
                let _ = gpu.finish_supervised(job_id, JobState::Failed, now_ms());
                return Err(error);
            }
        }
        let result = self.start_generic(id, operation_id, launch).await;
        if result.is_err() {
            let mut runtime = self.runtime.lock().await;
            if !runtime.registry.get(id)?.state.is_terminal() {
                runtime
                    .registry
                    .transition(id, SessionState::Failed, "gpu_start_failed")?;
            }
            drop(runtime);
            // Retry the exact journaled scope cleanup before releasing capacity.
            let _ = self.cleanup_session(id).await;
        }
        result
    }

    pub(crate) async fn finish_gpu(&self, id: Uuid, succeeded: bool) -> Result<()> {
        let session = self.runtime.lock().await.registry.get(id)?;
        if session.execution.cleanup_pending || !session.execution.gpu_cleanup_pending {
            return Ok(());
        }
        if let Some(job_id) = session.execution.gpu_job_id {
            let state = if succeeded {
                JobState::Completed
            } else if session.state == SessionState::Cancelled {
                JobState::Cancelled
            } else {
                JobState::Failed
            };
            let mut gpu = self.gpu.lock().await;
            ensure!(
                gpu.jobs.get(&job_id).is_some_and(|job| job.supervised),
                "supervised GPU lease is missing"
            );
            if gpu.jobs.get(&job_id).is_some_and(|job| job.cleanup_pending) {
                gpu.finish_supervised(job_id, state, now_ms())?;
            }
            drop(gpu);
            self.runtime.lock().await.registry.update(id, |s| {
                s.execution.gpu_cleanup_pending = false;
                Registry::record_event(s, "gpu_execution_released");
                Ok(())
            })?;
        }
        Ok(())
    }
}
