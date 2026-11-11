use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ResourceBudget {
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
        if !(1..=10_000).contains(&self.cpu_weight)
            || self.memory_high_bytes == 0
            || self.memory_high_bytes > self.memory_max_bytes
            || self.tasks_max == 0
            || self.tasks_max > 4096
            || self.max_subagents > 32
            || self.inference_concurrency > 32
            || self.timeout_seconds == 0
            || self.timeout_seconds > 86400
        {
            return Err("invalid resource budget".into());
        }
        Ok(())
    }

    #[must_use]
    pub fn fits_within(&self, parent: &Self) -> bool {
        self.cpu_weight <= parent.cpu_weight
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

impl RuntimeSpec {
    /// # Errors
    /// Returns an error for invalid identities, capabilities, budgets, or task metadata.
    pub fn validate(&self) -> Result<(), String> {
        self.budget.validate()?;
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
        if self.capabilities.len() > 32
            || self.capabilities.iter().any(|cap| {
                !matches!(
                    cap.kind.as_str(),
                    "fs.read" | "fs.write" | "session.spawn" | "tool.exec" | "gpu.use"
                ) || cap.target.is_empty()
                    || cap.target.len() > 4096
                    || cap.target.chars().any(char::is_control)
                    || cap
                        .limit
                        .is_some_and(|limit| limit > 32 || cap.kind != "session.spawn")
            })
        {
            return Err(
                "invalid Guard grants; inference/egress/credentials require a reviewed bridge"
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
pub struct ExecutionRecord {
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
    pub guard_cleanup_pending: bool,
    pub guard_profile_digest: Option<String>,
    pub workspace_preparing: bool,
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
