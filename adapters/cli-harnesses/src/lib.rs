//! Pinned native CLI launch plans; the supervisor retains process and policy ownership.
pub mod protocol;
use cerynth_agent_api::{
    AdapterCapability, AdapterDescriptor, AdapterInput, HarnessAdapter, HarnessKind, LaunchRequest,
    Session, Unsupported,
};
use std::path::PathBuf;

pub struct CliHarnessAdapter {
    pub kind: HarnessKind,
    /// Resolved by the trusted owner; never supplied by an inference response.
    pub program: PathBuf,
}

#[async_trait::async_trait]
impl HarnessAdapter for CliHarnessAdapter {
    fn descriptor(&self) -> AdapterDescriptor {
        AdapterDescriptor {
            kind: self.kind,
            name: format!("{:?}", self.kind),
            implementation: "supervised_cli_jsonl".into(),
            capabilities: if matches!(
                self.kind,
                HarnessKind::OpenCode
                    | HarnessKind::Hermes
                    | HarnessKind::Codex
                    | HarnessKind::ClaudeCode
            ) {
                vec![
                    AdapterCapability::Launch,
                    AdapterCapability::StructuredEvents,
                    AdapterCapability::Cancellation,
                ]
            } else {
                Vec::new()
            },
        }
    }
    async fn check_installation(&self) -> Result<bool, Unsupported> {
        Ok(self.program.is_absolute() && self.program.is_file())
    }
    async fn prepare_launch(
        &self,
        session: &Session,
        input: AdapterInput,
    ) -> Result<LaunchRequest, Unsupported> {
        if self.kind != session.spec.harness || !self.program.is_absolute() {
            return Err(Unsupported("adapter identity or executable mismatch"));
        }
        let AdapterInput::Prompt(input) = input else {
            return Err(Unsupported("native CLI requires a prompt"));
        };
        if input.trim().is_empty() || input.len() > cerynth_agent_api::MAX_PROMPT_BYTES {
            return Err(Unsupported("invalid native harness prompt"));
        }
        self.prepare(session, input)
    }
}

impl CliHarnessAdapter {
    fn prepare(&self, session: &Session, input: String) -> Result<LaunchRequest, Unsupported> {
        let model_name = if session.spec.harness == HarnessKind::OpenCode {
            format!("ollama/{}", session.spec.model.name)
        } else {
            session.spec.model.name.clone()
        };
        let model = &model_name;
        let (_name, args) = match session.spec.harness {
            HarnessKind::Codex => (
                "codex",
                vec![
                    "--no-daemon",
                    "-c",
                    "model_provider=\"ollama\"",
                    "exec",
                    "--oss",
                    "--local-provider",
                    "ollama",
                    "--json",
                    "--ephemeral",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--sandbox",
                    "workspace-write",
                    "--model",
                    model,
                    "-",
                ],
            ),
            HarnessKind::ClaudeCode => (
                "claude",
                vec![
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--model",
                    model,
                    "--permission-mode",
                    "dontAsk",
                ],
            ),
            HarnessKind::OpenCode => (
                "opencode",
                vec!["run", "--pure", "--format", "json", "--model", model],
            ),
            HarnessKind::Hermes => (
                "hermes",
                vec![
                    "chat",
                    "--query-file",
                    "-",
                    "--oneshot",
                    "--format",
                    "stream-json",
                    "--provider",
                    "custom",
                    "--base-url",
                    "http://127.0.0.1:11434/v1",
                    "--reasoning",
                    "none",
                    "--safe-mode",
                    "--max-turns",
                    "8",
                    "--model",
                    model,
                ],
            ),
            _ => return Err(Unsupported("unsupported native CLI harness")),
        };
        let mut args: Vec<String> = args.into_iter().map(str::to_owned).collect();
        if session.spec.runtime.hermes_terminal_profile {
            // The managed profile is the only visible config. Rules are bypassed;
            // the read-only profile enables no added plugins, MCP or hooks.
            if let Some(flag) = args.iter_mut().find(|arg| arg.as_str() == "--safe-mode") {
                *flag = "--ignore-rules".into();
            }
            args.extend(["--toolsets".into(), "terminal".into()]);
        }
        Ok(LaunchRequest {
            program: self.program.clone(),
            args,
            initial_input: Some(input),
            pty: false,
        })
    }
}
