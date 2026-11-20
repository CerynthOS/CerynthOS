//! Opt-in versioned resource foundation through systemd, separate from scheduler IPC.
use crate::{AgentService, process::PreparedLaunch, registry::Registry};
use anyhow::{Context, Result, ensure};
use cerynth_agent_api::{ResourceAllocation, ResourceBudget, SessionState};
use cerynth_resources::{
    admission::{self, Capacity},
    systemd::{self, Hierarchy},
};
use serde_json::{Value, json};
use std::path::Path;
use uuid::Uuid;

fn reserved_scope(sessions: &[cerynth_agent_api::Session], parent: Option<Uuid>) -> Capacity {
    sessions
        .iter()
        .filter_map(|s| s.execution.resource_allocation.as_ref())
        .filter(|a| a.active && a.parent_session_id == parent)
        .fold(
            Capacity {
                memory_bytes: 0,
                cpu_quota_micros_per_second: 0,
            },
            |mut total, a| {
                total.memory_bytes = total
                    .memory_bytes
                    .saturating_add(a.granted.memory_max_bytes);
                total.cpu_quota_micros_per_second = total
                    .cpu_quota_micros_per_second
                    .saturating_add(a.granted.cpu_quota_micros_per_second.unwrap_or(0));
                total
            },
        )
}

pub(crate) fn from_environment(directory: &Path) -> Result<Option<Hierarchy>> {
    match std::env::var("CERYNTH_RESOURCE_HIERARCHY").as_deref() {
        Err(_) | Ok("0") => return Ok(None),
        Ok("1") => {}
        Ok(_) => anyhow::bail!("CERYNTH_RESOURCE_HIERARCHY must be 0 or 1"),
    }
    require_validated_cpu_scheduler()?;
    let memory = cerynth_resources::total_memory().context("physical memory capacity unknown")?;
    let configured = std::env::var("CERYNTH_AGENT_MEMORY_MAX_BYTES")
        .map_or(Ok(memory / 2), |s| s.parse::<u64>())?;
    ensure!(
        configured >= 64 * 1024 * 1024 && configured <= memory.saturating_sub(512 * 1024 * 1024),
        "managed memory capacity must preserve physical system headroom"
    );
    let cpus = std::thread::available_parallelism()?.get();
    let quota = u64::try_from(cpus.saturating_sub(2).max(1))? * 1_000_000;
    Ok(Some(Hierarchy::new(
        &directory.canonicalize()?,
        configured,
        quota,
    )))
}

fn require_validated_cpu_scheduler() -> Result<()> {
    match std::fs::read_to_string("/sys/kernel/sched_ext/state") {
        Ok(state) => ensure!(
            state.trim() == "disabled",
            "experimental nested CPU accounting requires sched_ext disabled until Person 1 qualifies its CPU policy"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn local_admission(
    hierarchy: &Hierarchy,
    registry: &Registry,
    source: &cerynth_agent_api::Session,
    minimum: Option<&ResourceBudget>,
) -> Result<admission::Decision> {
    if let Some(parent) = source.spec.runtime.parent_id {
        let parent = registry.get(parent)?;
        ensure!(
            parent
                .execution
                .resource_allocation
                .as_ref()
                .is_some_and(|a| a.active),
            "parent lacks an active allocation"
        );
        ensure!(
            source
                .spec
                .runtime
                .budget
                .fits_within(&parent.spec.runtime.budget),
            "child exceeds parent allocation"
        );
        admission::decide(
            &source.spec.runtime.budget,
            minimum,
            &Capacity {
                memory_bytes: parent.spec.runtime.budget.memory_max_bytes,
                cpu_quota_micros_per_second: parent
                    .spec
                    .runtime
                    .budget
                    .cpu_quota_micros_per_second
                    .unwrap_or(hierarchy.cpu_quota_micros_per_second),
            },
            &reserved_scope(&registry.list(), Some(parent.id)),
            cerynth_resources::pressure::sample().blocks_new_work(crate::registry::now_ms()),
        )
    } else {
        let reserved = reserved_scope(&registry.list(), None);
        admission::decide(
            &source.spec.runtime.budget,
            minimum,
            &Capacity {
                memory_bytes: hierarchy.memory_max_bytes,
                cpu_quota_micros_per_second: hierarchy.cpu_quota_micros_per_second,
            },
            &reserved,
            cerynth_resources::pressure::sample().blocks_new_work(crate::registry::now_ms()),
        )
    }
}

impl AgentService {
    pub(crate) async fn plan_global_resources(
        &self,
        id: Uuid,
        minimum: Option<ResourceBudget>,
        gpu_device_id: Option<String>,
    ) -> Result<Value> {
        use cerynth_resources::global::{Demand, OwnerSnapshot};
        let session = self.runtime.lock().await.registry.get(id)?;
        let memory =
            cerynth_resources::total_memory().context("physical memory capacity unknown")?;
        let cpus = u64::try_from(std::thread::available_parallelism()?.get())?;
        let reserved = reserved_scope(&self.runtime.lock().await.registry.list(), None);
        let request = Demand::for_session(&session, minimum, gpu_device_id);
        let snapshot = OwnerSnapshot {
            observed_at_ms: crate::registry::now_ms(),
            complete_managed_workload_accounting: false,
            capacity: Capacity {
                memory_bytes: memory / 2,
                cpu_quota_micros_per_second: cpus.saturating_sub(2).max(1) * 1_000_000,
            },
            reserved,
            parent_authority: None,
            accelerators: std::collections::BTreeMap::default(),
            inference_limit: 4,
            inference_reserved: 0,
            pressure_blocks_new_work: self
                .platform_pressure()
                .await
                .blocks_new_work(crate::registry::now_ms()),
        };
        let proposal =
            cerynth_resources::global::plan(&request, &snapshot, crate::registry::now_ms())?;
        Ok(
            json!({"request":request,"proposal":proposal,"platform_request":cerynth_resources::platform::request(&session)?,
            "accounting_scope":"instance_only; global authority has not attested all services","applied":false}),
        )
    }
    pub(crate) async fn resource_view(&self) -> Result<Value> {
        let pressure = self.platform_pressure().await;
        let runtime = self.runtime.lock().await;
        let allocations: Vec<_> = runtime
            .registry
            .list()
            .iter()
            .filter_map(|s| {
                s.execution
                    .resource_allocation
                    .as_ref()
                    .map(|a| json!({"session_id":s.id,"allocation":a}))
            })
            .collect();
        Ok(
            json!({"version":1,"hierarchy_enabled":self.resources.is_some(),"pressure":pressure,
            "pressure_blocks_new_work":pressure.blocks_new_work(crate::registry::now_ms()),
            "allocations":allocations,"enforcement":"systemd slices when enabled; cooperative GPU reservations",
            "shared_authority":self.shared_resources.as_ref().map(|c|c.authority.view()).transpose()?,
            "shared_accounting_scope":"participating agentd root subtrees only; backend servers and bypassing workloads are outside this domain"}),
        )
    }
    pub(crate) async fn admit_resources(
        &self,
        id: Uuid,
        minimum: Option<&ResourceBudget>,
    ) -> Result<Value> {
        let hierarchy = self
            .resources
            .as_ref()
            .context("enable the owner resource hierarchy first")?;
        let mut runtime = self.runtime.lock().await;
        let source = runtime.registry.get(id)?;
        if let Some(allocation) = &source.execution.resource_allocation {
            if let Some(client) = &self.shared_resources {
                if source.spec.runtime.parent_id.is_none() {
                    client.verify(&source)?;
                }
            }
            ensure!(
                allocation.active,
                "allocation was released; create a new session"
            );
            ensure!(
                minimum.is_none_or(|minimum| minimum.fits_within(&allocation.granted)),
                "existing allocation is below requested minimum"
            );
            return Ok(json!({"decision":"accept","allocation":allocation,"existing":true}));
        }
        ensure!(
            source.state == SessionState::Created,
            "admit before starting work"
        );
        ensure!(
            !runtime
                .registry
                .list()
                .iter()
                .any(|child| child.spec.runtime.parent_id == Some(id)),
            "admit the parent before creating its children"
        );
        let mut decision = local_admission(hierarchy, &runtime.registry, &source, minimum)?;
        if source.spec.runtime.parent_id.is_none() && self.shared_resources.is_some() {
            if let Some(granted) = decision.granted() {
                decision = self.reserve_shared(&source, granted, minimum).await?;
                if let Some(granted) = decision.granted().cloned() {
                    decision = if granted == source.spec.runtime.budget {
                        admission::Decision::Accept { granted }
                    } else {
                        admission::Decision::Reduce { granted }
                    };
                }
            }
        }
        let mut response = serde_json::to_value(&decision)?;
        if let Some(granted) = decision.granted() {
            let allocation = ResourceAllocation {
                id: Uuid::new_v4(),
                parent_session_id: source.spec.runtime.parent_id,
                granted: granted.clone(),
                active: true,
                kernel_enforced: false,
            };
            runtime.registry.update(id, |s| {
                s.spec.runtime.budget = granted.clone();
                s.execution.resource_allocation = Some(allocation.clone());
                Registry::record_event(s, "resources_reserved");
                Ok(())
            })?;
            response["allocation"] = serde_json::to_value(allocation)?;
        }
        Ok(response)
    }

    pub(crate) async fn bind_resources(&self, id: Uuid, plan: &mut PreparedLaunch) -> Result<()> {
        let Some(hierarchy) = &self.resources else {
            return Ok(());
        };
        let _writer = self.resource_lock.lock().await;
        require_validated_cpu_scheduler()?;
        let (session, unit) = {
            let mut runtime = self.runtime.lock().await;
            let session = runtime.registry.get(id)?;
            if let Some(client) = &self.shared_resources {
                let mut root = session.clone();
                while let Some(parent) = root.spec.runtime.parent_id {
                    root = runtime.registry.get(parent)?;
                }
                client.verify(&root)?;
            }
            cerynth_resources::platform::request(&session)?;
            let allocation = session
                .execution
                .resource_allocation
                .as_ref()
                .context("session was not admitted")?;
            ensure!(
                allocation.active && session.state == SessionState::Starting,
                "allocation is not launchable"
            );
            let parent = session
                .spec
                .runtime
                .parent_id
                .map(|id| runtime.registry.get(id))
                .transpose()?;
            ensure!(
                parent.as_ref().is_none_or(|s| s
                    .execution
                    .resource_allocation
                    .as_ref()
                    .is_some_and(|a| a.kernel_enforced)),
                "parent kernel boundary is not ready"
            );
            let parent_slice = parent
                .as_ref()
                .and_then(|s| s.execution.resource_slice.as_deref());
            ensure!(
                parent.is_none() || parent_slice.is_some(),
                "start the parent's resource boundary before the child"
            );
            let unit = hierarchy.session(&session.spec.runtime.project, parent_slice, id)?;
            runtime.registry.update(id, |s| {
                s.execution.resource_slice = Some(unit.clone());
                s.execution.resource_cleanup_pending = true;
                Registry::record_event(s, "resource_slice_preparing");
                Ok(())
            })?;
            (session, unit)
        };
        hierarchy
            .ensure(
                &session.spec.runtime.project,
                &unit,
                &session.spec.runtime.budget,
            )
            .await?;
        let separator = plan
            .args
            .iter()
            .position(|arg| arg == "--")
            .context("owned systemd wrapper required")?;
        plan.args.insert(separator, format!("--slice={unit}"));
        self.runtime.lock().await.registry.update(id, |s| {
            s.execution
                .resource_allocation
                .as_mut()
                .context("allocation disappeared")?
                .kernel_enforced = true;
            Registry::record_event(s, "resource_slice_ready");
            Ok(())
        })?;
        Ok(())
    }

    pub(crate) async fn resource_accounting(&self, id: Uuid) -> Result<Value> {
        let session = self.runtime.lock().await.registry.get(id)?;
        let unit = session
            .execution
            .resource_slice
            .context("session has no resource slice")?;
        Ok(serde_json::to_value(systemd::accounting(&unit).await?)?)
    }

    pub(crate) async fn platform_resources(&self, id: Uuid) -> Result<Value> {
        let session = self.runtime.lock().await.registry.get(id)?;
        let unit = session
            .execution
            .resource_slice
            .as_ref()
            .context("session has no resource slice")?;
        let group = systemd::accounting(unit).await?.control_group;
        let path = std::path::PathBuf::from("/sys/fs/cgroup").join(group.trim_start_matches('/'));
        Ok(serde_json::to_value(
            cerynth_resources::platform::snapshot(&session, &path)?,
        )?)
    }

    pub(crate) async fn platform_pressure(&self) -> cerynth_resources::pressure::Sample {
        let mut sample = cerynth_resources::pressure::sample();
        let roots: Vec<_> = self
            .runtime
            .lock()
            .await
            .registry
            .list()
            .into_iter()
            .filter(|s| {
                s.spec.runtime.parent_id.is_none()
                    && s.execution
                        .resource_allocation
                        .as_ref()
                        .is_some_and(|a| a.active && a.kernel_enforced)
            })
            .collect();
        for session in roots {
            if let Ok(value) = self.platform_resources(session.id).await {
                if let Ok(usage) = serde_json::from_value::<
                    cerynth_resources::platform::ResourceUsage,
                >(value["usage"].clone())
                {
                    cerynth_resources::platform::pressure(&mut sample, &usage);
                } else {
                    sample.available_memory_bytes = None;
                }
            } else {
                // A missing owned boundary cannot be interpreted as zero pressure.
                sample.available_memory_bytes = None;
            }
        }
        sample
    }

    pub(crate) async fn release_resources(&self) -> Result<()> {
        let _writer = self.resource_lock.lock().await;
        loop {
            let sessions = self.runtime.lock().await.registry.list();
            let eligible = sessions.iter().find(|s| {
                s.state.is_terminal()
                    && !s.execution.cleanup_pending
                    && !s.execution.workspace_preparing
                    && !s.execution.gpu_cleanup_pending
                    && !s.execution.model_residency_cleanup_pending
                    && !s.execution.guard_cleanup_pending
                    && s.execution
                        .resource_allocation
                        .as_ref()
                        .is_some_and(|a| a.active)
                    && !sessions.iter().any(|child| {
                        child.spec.runtime.parent_id == Some(s.id)
                            && child
                                .execution
                                .resource_allocation
                                .as_ref()
                                .is_some_and(|a| a.active)
                    })
            });
            let Some(session) = eligible else {
                return self.reconcile_shared_resources().await;
            };
            if let Some(unit) = &session.execution.resource_slice {
                systemd::stop(unit).await?;
            }
            if session.spec.runtime.parent_id.is_none() {
                if let Some(client) = &self.shared_resources {
                    self.revoke_shared_inference(session.id).await?;
                    client.authority.release(&client.owner, session.id)?;
                }
            }
            self.runtime.lock().await.registry.update(session.id, |s| {
                s.execution.resource_cleanup_pending = false;
                s.execution
                    .resource_allocation
                    .as_mut()
                    .context("allocation missing")?
                    .active = false;
                Registry::record_event(s, "resources_released");
                Ok(())
            })?;
        }
    }
}
