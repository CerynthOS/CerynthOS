use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ResourceBudget {
    /// Optional owner-selected platform constraints, enforced before execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<crate::PlatformLimits>,
    /// Capacity ceiling for the bounded workspace and its managed child volumes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_max_bytes: Option<u64>,
    /// Hard CPU bandwidth in microseconds per second; None means weight-only sharing.
    pub cpu_quota_micros_per_second: Option<u64>,
    pub cpu_weight: u32,
    pub memory_high_bytes: u64,
    pub memory_max_bytes: u64,
    pub tasks_max: u32,
    pub max_subagents: u32,
    pub inference_concurrency: u32,
    pub gpu_memory_bytes: u64,
    pub timeout_seconds: u64,
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self {
            platform: None,
            workspace_max_bytes: None,
            cpu_quota_micros_per_second: None,
            cpu_weight: 100,
            memory_high_bytes: 1024 * 1024 * 1024,
            memory_max_bytes: 2 * 1024 * 1024 * 1024,
            tasks_max: 128,
            max_subagents: 4,
            inference_concurrency: 2,
            gpu_memory_bytes: 0,
            timeout_seconds: 300,
        }
    }
}

impl ResourceBudget {
    /// # Errors
    /// Returns an error if limits are zero, inconsistent, or outside supported ranges.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(platform) = &self.platform {
            platform.validate()?;
        }
        if !(1..=10_000).contains(&self.cpu_weight)
            || self.cpu_quota_micros_per_second.is_some_and(|quota| {
                !(10_000..=4_096_000_000).contains(&quota) || quota % 10_000 != 0
            })
            || self.memory_high_bytes == 0
            || self.memory_high_bytes > self.memory_max_bytes
            || self.tasks_max == 0
            || self.tasks_max > 4096
            || self.max_subagents > 32
            || self.inference_concurrency > 32
            || self.timeout_seconds == 0
            || self.timeout_seconds > 86400
            || self
                .workspace_max_bytes
                .is_some_and(|bytes| bytes < 16 * 1024 * 1024)
        {
            return Err("invalid resource budget".into());
        }
        Ok(())
    }

    #[must_use]
    pub fn fits_within(&self, parent: &Self) -> bool {
        self.platform
            .as_ref()
            .unwrap_or(&crate::PlatformLimits::default())
            .fits_within(
                parent
                    .platform
                    .as_ref()
                    .unwrap_or(&crate::PlatformLimits::default()),
            )
            && match (self.workspace_max_bytes, parent.workspace_max_bytes) {
                (_, None) => true,
                (Some(child), Some(parent)) => child <= parent,
                (None, Some(_)) => false,
            }
            && self.cpu_weight <= parent.cpu_weight
            && match (
                self.cpu_quota_micros_per_second,
                parent.cpu_quota_micros_per_second,
            ) {
                (_, None) => true,
                (Some(child), Some(parent)) => child <= parent,
                (None, Some(_)) => false,
            }
            && self.memory_max_bytes <= parent.memory_max_bytes
            && self.memory_high_bytes <= parent.memory_high_bytes
            && self.tasks_max <= parent.tasks_max
            && self.max_subagents <= parent.max_subagents
            && self.inference_concurrency <= parent.inference_concurrency
            && self.gpu_memory_bytes <= parent.gpu_memory_bytes
            && self.timeout_seconds <= parent.timeout_seconds
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Managed,
    #[default]
    Integrated,
    Observed,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    #[default]
    Deny,
    Host,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum IsolationPolicy {
    #[default]
    Required,
    Unisolated,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadPhase {
    #[default]
    Idle,
    Inference,
    Tool,
    Compile,
    Batch,
    ApprovalWait,
    Cleanup,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeSpec {
    /// Owner-selected coding profile for small local models; OS policy still
    /// enforces the boundary independently of the harness tool list.
    pub hermes_terminal_profile: bool,
    pub transport: HarnessTransport,
    pub project: String,
    pub parent_id: Option<Uuid>,
    pub task: Option<TaskAssignment>,
    pub budget: ResourceBudget,
    pub capabilities: BTreeSet<String>,
    pub message_peers: BTreeSet<Uuid>,
    pub network: NetworkPolicy,
    pub isolation: IsolationPolicy,
    pub require_cgroup: bool,
    /// A present policy requires Person 2's Guard profile; never falls back.
    pub guard: Option<GuardPolicy>,
}

impl Default for RuntimeSpec {
    fn default() -> Self {
        Self {
            hermes_terminal_profile: false,
            transport: HarnessTransport::CliJsonl,
            project: "default".into(),
            parent_id: None,
            task: None,
            budget: ResourceBudget::default(),
            capabilities: BTreeSet::new(),
            message_peers: BTreeSet::new(),
            network: NetworkPolicy::Deny,
            isolation: IsolationPolicy::Required,
            require_cgroup: true,
            guard: None,
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum HarnessTransport {
    #[default]
    CliJsonl,
    OpenCodeHttp,
    OpenCodeAcp,
}

impl RuntimeSpec {
    /// # Errors
    /// Returns an error for invalid identities, capabilities, budgets, or task metadata.
    pub fn validate(&self) -> Result<(), String> {
        self.budget.validate()?;
        if self.budget.workspace_max_bytes.is_some()
            && (self.isolation != IsolationPolicy::Required || !self.require_cgroup)
        {
            return Err("bounded workspace requests require an isolated managed session".into());
        }
        if self.budget.platform.is_some() && !self.require_cgroup {
            return Err("advanced platform constraints require cgroup supervision".into());
        }
        if let Some(guard) = &self.guard {
            guard.validate()?;
            if self.isolation != IsolationPolicy::Required
                || !self.require_cgroup
                || self.network != NetworkPolicy::Deny
            {
                return Err(
                    "Guard profiles require isolation, a cgroup and denied host networking".into(),
                );
            }
        }
        if self.project.is_empty()
            || self.project.len() > 128
            || self.project.chars().any(char::is_control)
            || self.capabilities.len() > 32
            || self.message_peers.len() > 128
            || self
                .capabilities
                .iter()
                .any(|c| c.len() > 128 || c.chars().any(char::is_control))
        {
            return Err("invalid runtime metadata".into());
        }
        if let Some(task) = &self.task {
            task.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuardPolicy {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub remotes: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub native_inference: bool,
    #[serde(default)]
    pub capabilities: Vec<GuardCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuardCapability {
    pub kind: String,
    pub target: String,
    #[serde(default)]
    pub mode: GuardCapabilityMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum GuardCapabilityMode {
    #[default]
    Allow,
    Approval,
    Deny,
}

impl GuardPolicy {
    /// # Errors
    /// Rejects unsupported grants; this integration currently supports offline profiles.
    pub fn validate(&self) -> Result<(), String> {
        if self.remotes.len() > 16
            || self.remotes.iter().any(|(name, url)| {
                name.is_empty()
                    || name.len() > 64
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
                    || url.is_empty()
                    || url.len() > 4096
                    || url.chars().any(char::is_control)
                    || url.contains(['?', '#'])
                    || !(url.starts_with('/')
                        || url.starts_with("file:///")
                        || url.starts_with("https://")
                        || url.starts_with("ssh://"))
                    || (url.starts_with("https://") && url.contains('@'))
                    || (url.starts_with("ssh://")
                        && url.contains('@')
                        && url
                            .split('@')
                            .next()
                            .is_some_and(|u| u.trim_start_matches("ssh://").contains(':')))
            })
        {
            return Err("invalid owner Git remotes; embedded credentials and unsupported schemes are forbidden".into());
        }
        if self.capabilities.len() > 32
            || self.capabilities.iter().any(|cap| {
                !(matches!(
                    cap.kind.as_str(),
                    "fs.read" | "fs.write" | "session.spawn" | "tool.exec" | "gpu.use" | "git.push"
                ) || (self.native_inference
                    && matches!(
                        (cap.kind.as_str(), cap.target.as_str()),
                        ("inference.use", "cerynth") | ("credential.use", "inference-session")
                    )))
                    || cap.target.is_empty()
                    || cap.target.len() > 4096
                    || cap.target.chars().any(char::is_control)
                    || cap
                        .limit
                        .is_some_and(|limit| limit > 32 || cap.kind != "session.spawn")
            })
        {
            return Err(
                "invalid Guard grants; native inference requires its explicit supported target and credential"
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskAssignment {
    pub id: Uuid,
    pub objective: String,
    pub completion_criteria: String,
    pub expected_output: String,
    #[serde(default)]
    pub dependencies: BTreeSet<Uuid>,
}

impl TaskAssignment {
    /// # Errors
    /// Returns an error for empty or oversized task fields.
    pub fn validate(&self) -> Result<(), String> {
        if self.objective.trim().is_empty()
            || self.objective.len() > 8192
            || self.completion_criteria.is_empty()
            || self.completion_criteria.len() > 4096
            || self.expected_output.len() > 1024
            || self.dependencies.len() > 32
        {
            return Err("invalid task assignment".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchRequest {
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub initial_input: Option<String>,
    #[serde(default)]
    pub pty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_ticks: u64,
    pub boot_id: String,
    pub process_group: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
// Independent process, authorization, workspace and GPU cleanup journals can
// be pending simultaneously; these are not mutually exclusive lifecycle states.
#[allow(clippy::struct_excessive_bools)]
pub struct ExecutionRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_volume: Option<WorkspaceVolume>,
    pub model_residency: Option<ModelResidencyBinding>,
    pub model_residency_cleanup_pending: bool,
    pub resource_allocation: Option<ResourceAllocation>,
    pub resource_slice: Option<String>,
    pub resource_cleanup_pending: bool,
    pub retry: Option<RetryLineage>,
    pub retry_session_id: Option<Uuid>,
    pub gpu_job_id: Option<Uuid>,
    pub gpu_cleanup_pending: bool,
    pub gpu_authorization: Option<GpuAuthorizationReceipt>,
    pub mode: SessionMode,
    pub process: Option<ProcessIdentity>,
    pub allocation_id: Option<Uuid>,
    pub cgroup_unit: Option<String>,
    pub exit_code: Option<i32>,
    pub failure_reason: Option<String>,
    pub phase: WorkloadPhase,
    pub operations: Vec<OperationRecord>,
    pub messages: Vec<AgentMessage>,
    pub artifacts: Vec<Artifact>,
    pub worktree: Option<PathBuf>,
    pub harness_events: Vec<NormalizedHarnessEvent>,
    pub cleanup_pending: bool,
    pub guard_socket: Option<PathBuf>,
    pub native_guard_unit: Option<String>,
    pub guard_cleanup_pending: bool,
    pub guard_profile_digest: Option<String>,
    pub workspace_preparing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceVolume {
    pub volume_id: Uuid,
    pub parent_volume_id: Option<Uuid>,
    pub maximum_image_bytes: u64,
    pub subtree_maximum_bytes: u64,
    pub filesystem_capacity_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceAllocation {
    pub id: Uuid,
    pub parent_session_id: Option<Uuid>,
    pub granted: ResourceBudget,
    pub active: bool,
    pub kernel_enforced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelResidencyBinding {
    pub lease_id: Uuid,
    pub endpoint_id: String,
    pub model: String,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetryLineage {
    pub root_session_id: Uuid,
    pub previous_session_id: Uuid,
    /// Three owner-sponsored retries maximum; no automatic process replay.
    pub attempt: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GpuAuthorizationReceipt {
    pub request_id: Uuid,
    pub job_id: Uuid,
    pub launch_sha256: String,
    /// Guard's policy-decision label; the runtime separately gates owned launch.
    pub policy_enforcement: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NormalizedHarnessEvent {
    pub sequence: u64,
    pub kind: String,
    pub tool_call_id: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub cached_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_write_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Started,
    Completed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OperationRecord {
    pub id: Uuid,
    pub fingerprint: String,
    pub status: OperationStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentMessage {
    pub id: Uuid,
    pub sender: Uuid,
    pub recipient: Uuid,
    pub timestamp_ms: u64,
    pub content: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    File,
    Diff,
    TestResult,
    TaskResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Artifact {
    pub id: Uuid,
    pub path: PathBuf,
    pub kind: ArtifactKind,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UnifiedEvent {
    pub schema_version: u32,
    pub event_id: Uuid,
    pub trace_id: Uuid,
    pub session_id: Uuid,
    pub parent_session_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub timestamp_ms: u64,
    pub sequence: u64,
    pub category: String,
    pub name: String,
    pub attributes: serde_json::Value,
}
