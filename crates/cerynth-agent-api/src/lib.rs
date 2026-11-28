//! Draft Person 3 contracts. Independent of the frozen scheduler IPC protocol.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod contract;
mod platform_limits;
mod runtime;
pub use platform_limits::{DeviceIoLimit, PlatformLimits};
pub use runtime::*;

pub const API_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_PROMPT_BYTES: usize = 32 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Created,
    Preparing,
    Starting,
    Running,
    WaitingForInference,
    WaitingForTool,
    WaitingForApproval,
    Pausing,
    Paused,
    Resuming,
    Completing,
    Completed,
    Failed,
    Cancelled,
    Recovering,
}

impl SessionState {
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        use SessionState::{
            Cancelled, Completed, Completing, Created, Failed, Paused, Pausing, Preparing,
            Recovering, Resuming, Running, Starting, WaitingForApproval, WaitingForInference,
            WaitingForTool,
        };
        if self.is_terminal() || self == next {
            return false;
        }
        if matches!(next, Failed | Cancelled | Recovering) {
            return true;
        }
        matches!(
            (self, next),
            (Created, Preparing)
                | (Preparing | Resuming | Recovering, Starting)
                | (Starting, Running)
                | (
                    Running,
                    WaitingForInference
                        | WaitingForTool
                        | WaitingForApproval
                        | Pausing
                        | Completing
                )
                | (
                    WaitingForInference | WaitingForTool | WaitingForApproval,
                    Running | Pausing
                )
                | (Pausing, Paused)
                | (Paused, Resuming)
                | (Completing, Completed)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind {
    /// Endpoint smoke test, with no tools or agent process.
    InferenceProbe,
    GenericPty,
    OpenCode,
    Hermes,
    Codex,
    ClaudeCode,
    Acp,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    /// Exact backend model identity, without a harness-provider prefix or credential.
    pub name: String,
    #[serde(default)]
    pub mode: ModelMode,
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub cloud_consent: bool,
    #[serde(default)]
    pub disable_thinking: bool,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ModelMode {
    #[default]
    Local,
    Cloud,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionSpec {
    pub name: String,
    pub harness: HarnessKind,
    pub workspace: PathBuf,
    pub model: ModelConfig,
    #[serde(default)]
    pub runtime: RuntimeSpec,
}

impl SessionSpec {
    /// Validate metadata. Workspace access is checked by the runtime.
    ///
    /// # Errors
    /// Returns an error for empty, oversized, or control-character metadata.
    pub fn validate(&self) -> Result<(), String> {
        for (label, value) in [("name", &self.name), ("model", &self.model.name)] {
            if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
                return Err(format!(
                    "{label} must contain 1..256 bytes without control characters"
                ));
            }
        }
        if !self.workspace.is_absolute() {
            return Err("workspace must be an absolute path".into());
        }
        self.runtime.validate()?;
        if self.runtime.hermes_terminal_profile && self.harness != HarnessKind::Hermes {
            return Err("Hermes terminal profile requires the Hermes harness".into());
        }
        if self.runtime.transport != HarnessTransport::CliJsonl
            && self.harness != HarnessKind::OpenCode
        {
            return Err("this protocol transport requires the OpenCode harness".into());
        }
        if let Some(guard) = &self.runtime.guard {
            if guard.native_inference {
                if self.harness != HarnessKind::OpenCode
                    || self.model.endpoint_id.is_none()
                    || self.runtime.parent_id.is_some()
                    || self.runtime.budget.max_subagents != 0
                    || guard
                        .capabilities
                        .iter()
                        .any(|c| matches!(c.kind.as_str(), "gpu.use" | "session.spawn"))
                    || !["inference.use", "credential.use"].iter().all(|kind| {
                        guard
                            .capabilities
                            .iter()
                            .any(|c| c.kind == *kind && c.mode == GuardCapabilityMode::Allow)
                    })
                {
                    return Err("native Guard inference requires a root OpenCode session, a configured endpoint, explicit inference/credential grants and max_subagents=0; delegation/GPU device access is unsupported".into());
                }
            } else if self.harness != HarnessKind::GenericPty {
                return Err("offline Guard profiles support generic_pty; select native_inference explicitly for OpenCode".into());
            }
        }
        if self.model.mode == ModelMode::Cloud && !self.model.cloud_consent {
            return Err("cloud model configuration requires explicit consent".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBoundary {
    /// Person 2's fail-closed profile, supervised inside a systemd scope.
    GuardSystemd,
    /// Metadata and endpoint requests only. No sandbox, cgroup, or GPU limit.
    UnenforcedPrototype,
    ProcessGroup,
    Bubblewrap,
    BubblewrapSystemd,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub id: Uuid,
    pub owner_uid: u32,
    pub spec: SessionSpec,
    pub state: SessionState,
    pub boundary: ExecutionBoundary,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub events: Vec<SessionEvent>,
    #[serde(default)]
    pub execution: ExecutionRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionEvent {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub state: SessionState,
    /// Fixed runtime reason codes; prompts and model responses are never persisted.
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdapterCapability {
    Launch,
    StructuredEvents,
    Cancellation,
    PauseResume,
    Subagents,
    ToolApprovals,
    Input,
    Discovery,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AdapterDescriptor {
    pub kind: HarnessKind,
    pub name: String,
    pub implementation: String,
    pub capabilities: Vec<AdapterCapability>,
}

#[derive(Debug, thiserror::Error)]
#[error("adapter operation is not supported: {0}")]
pub struct Unsupported(pub &'static str);

#[derive(Debug, Clone)]
pub enum AdapterInput {
    Prompt(String),
    Command(LaunchRequest),
}

/// Adapters translate supported inputs into launch plans without executing them.
/// The session supervisor owns start/input/stop, isolation and process accounting.
/// Descriptor capabilities describe the resulting supervised transport, not
/// independent permission to execute host processes or tools.
// async-trait generates a redundant must_use annotation on boxed futures.
#[allow(clippy::double_must_use)]
#[async_trait::async_trait]
pub trait HarnessAdapter: Send + Sync {
    fn descriptor(&self) -> AdapterDescriptor;

    async fn check_installation(&self) -> Result<bool, Unsupported> {
        Err(Unsupported("check_installation"))
    }

    async fn prepare_launch(
        &self,
        _session: &Session,
        _input: AdapterInput,
    ) -> Result<LaunchRequest, Unsupported> {
        Err(Unsupported("launch preparation"))
    }
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_contract: Option<String>,
    pub operation: Operation,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Protocol,
    PlanGlobalResources {
        id: Uuid,
        minimum: Option<ResourceBudget>,
        gpu_device_id: Option<String>,
    },
    ToolAwaitApproval {
        id: Uuid,
        approval_id: Uuid,
    },
    ToolDecide {
        id: Uuid,
        request: serde_json::Value,
    },
    ToolEffects {
        id: Uuid,
    },
    PlatformResources {
        id: Uuid,
    },
    Create {
        spec: SessionSpec,
    },
    Retry {
        id: Uuid,
        acknowledge_prior_effects: bool,
    },
    List,
    Status {
        id: Uuid,
    },
    Infer {
        id: Uuid,
        prompt: String,
    },
    Cancel {
        id: Uuid,
    },
    Models,
    Adapters,
    DiscoverHarness {
        harness: HarnessKind,
    },
    Start {
        id: Uuid,
        operation_id: Uuid,
        launch: LaunchRequest,
    },
    HarnessStart {
        id: Uuid,
        operation_id: Uuid,
        input: String,
    },
    Input {
        id: Uuid,
        input: String,
    },
    Stop {
        id: Uuid,
    },
    Output {
        id: Uuid,
        offset: u64,
        limit: usize,
    },
    Events {
        id: Uuid,
        after: u64,
    },
    ProcessTree {
        id: Uuid,
    },
    Delegate {
        parent: Uuid,
        spec: SessionSpec,
        task: TaskAssignment,
    },
    SendMessage {
        sender: Uuid,
        recipient: Uuid,
        content: String,
    },
    Messages {
        id: Uuid,
    },
    AuthorizeSharing {
        left: Uuid,
        right: Uuid,
    },
    Graph {
        project: String,
    },
    SetPhase {
        id: Uuid,
        phase: WorkloadPhase,
    },
    Worktree {
        id: Uuid,
    },
    CollectArtifact {
        id: Uuid,
        path: PathBuf,
        kind: ArtifactKind,
    },
    Artifacts {
        id: Uuid,
    },
    Diagnostics,
    ResourceAdmit {
        id: Uuid,
        minimum: Option<ResourceBudget>,
    },
    ResourceAccounting {
        id: Uuid,
    },
    ResourceView,
    Inventory,
    GpuSubmit {
        id: Uuid,
        device_id: String,
        memory_bytes: u64,
        priority: i32,
    },
    GpuJobs,
    GpuQuiesceModel {
        id: Uuid,
        lease_id: Uuid,
        endpoint_id: String,
        model: String,
    },
    GpuRun {
        id: Uuid,
        job_id: Uuid,
        operation_id: Uuid,
        launch: LaunchRequest,
    },
    GpuFinish {
        id: Uuid,
        job_id: Uuid,
        succeeded: bool,
    },
    RequestResources {
        id: Uuid,
        requested: ResourceBudget,
    },
    Observe {
        id: Uuid,
        pid: u32,
    },
    A2a {
        sponsor: Uuid,
        request: serde_json::Value,
    },
    Archive {
        id: Uuid,
    },
    Cleanup {
        id: Uuid,
    },
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u32,
    pub result: Result<serde_json::Value, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_rejects_terminal_resurrection_and_invalid_shortcuts() {
        assert!(!SessionState::Completed.can_transition_to(SessionState::Running));
        assert!(!SessionState::Cancelled.can_transition_to(SessionState::Preparing));
        assert!(!SessionState::Created.can_transition_to(SessionState::Completed));
        assert!(SessionState::WaitingForInference.can_transition_to(SessionState::Running));
        assert!(SessionState::WaitingForApproval.can_transition_to(SessionState::Cancelled));
    }

    #[test]
    fn unsupported_policy_fields_fail_closed() {
        let spec = serde_json::json!({"name":"test", "harness":"inference_probe",
            "workspace":"/tmp", "model":{"name":"test"}, "security":{"sandbox":"required"}});
        assert!(serde_json::from_value::<SessionSpec>(spec).is_err());
    }

    #[test]
    fn native_guard_requires_bounded_root_and_exact_gateway_authority() {
        let value = serde_json::json!({"name":"native", "harness":"open_code",
            "workspace":"/workspace", "model":{"name":"public-model", "endpoint_id":"local"},
            "runtime":{"guard":{"native_inference":true,"capabilities":[
                {"kind":"inference.use","target":"cerynth"},
                {"kind":"credential.use","target":"inference-session"}]},
                "budget":{"max_subagents":0}}});
        let spec: SessionSpec = serde_json::from_value(value.clone()).unwrap();
        assert!(spec.validate().is_ok());
        for change in [
            "harness",
            "parent",
            "children",
            "endpoint",
            "credential",
            "grant",
            "gpu",
        ] {
            let mut changed = value.clone();
            match change {
                "harness" => changed["harness"] = serde_json::json!("hermes"),
                "parent" => changed["runtime"]["parent_id"] = serde_json::json!(Uuid::new_v4()),
                "children" => changed["runtime"]["budget"]["max_subagents"] = serde_json::json!(1),
                "endpoint" => changed["model"]["endpoint_id"] = serde_json::Value::Null,
                "credential" => {
                    changed["runtime"]["guard"]["capabilities"][1]["target"] =
                        serde_json::json!("provider-key");
                }
                "grant" => {
                    changed["runtime"]["guard"]["capabilities"][0]["mode"] =
                        serde_json::json!("approval");
                }
                "gpu" => changed["runtime"]["guard"]["capabilities"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"kind":"gpu.use","target":"*"})),
                _ => unreachable!(),
            }
            assert!(
                serde_json::from_value::<SessionSpec>(changed)
                    .unwrap()
                    .validate()
                    .is_err(),
                "{change}"
            );
        }
    }
}
