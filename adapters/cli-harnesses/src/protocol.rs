//! Native protocol launch plans. The server remains inside the supervisor boundary.
use cerynth_agent_api::{
    AdapterCapability, AdapterDescriptor, AdapterInput, HarnessAdapter, HarnessKind,
    HarnessTransport, LaunchRequest, Session, Unsupported,
};
use std::path::PathBuf;

pub struct OpenCodeProtocolAdapter {
    pub native_program: PathBuf,
    pub bridge_program: PathBuf,
    pub transport: HarnessTransport,
}

#[async_trait::async_trait]
impl HarnessAdapter for OpenCodeProtocolAdapter {
    fn descriptor(&self) -> AdapterDescriptor {
        AdapterDescriptor {
            kind: HarnessKind::OpenCode,
            name: "opencode".into(),
            implementation: match self.transport {
                HarnessTransport::OpenCodeHttp => "supervised_http_api",
                HarnessTransport::OpenCodeAcp => "supervised_acp_v1",
                HarnessTransport::CliJsonl => "unsupported_protocol",
            }
            .into(),
            capabilities: if self.transport == HarnessTransport::CliJsonl {
                Vec::new()
            } else {
                vec![
                    AdapterCapability::Launch,
                    AdapterCapability::StructuredEvents,
                    AdapterCapability::Cancellation,
                ]
            },
        }
    }
    async fn check_installation(&self) -> Result<bool, Unsupported> {
        Ok(self.native_program.is_absolute()
            && self.native_program.is_file()
            && self.bridge_program.is_absolute()
            && self.bridge_program.is_file())
    }
    async fn prepare_launch(
        &self,
        session: &Session,
        input: AdapterInput,
    ) -> Result<LaunchRequest, Unsupported> {
        if session.spec.harness != HarnessKind::OpenCode
            || session.spec.runtime.transport != self.transport
            || !self.native_program.is_absolute()
            || !self.bridge_program.is_absolute()
        {
            return Err(Unsupported(
                "protocol adapter identity or executable mismatch",
            ));
        }
        let AdapterInput::Prompt(prompt) = input else {
            return Err(Unsupported("protocol adapter requires a prompt"));
        };
        if prompt.trim().is_empty() || prompt.len() > cerynth_agent_api::MAX_PROMPT_BYTES {
            return Err(Unsupported("invalid protocol prompt"));
        }
        let mode = match self.transport {
            HarnessTransport::OpenCodeHttp => "http",
            HarnessTransport::OpenCodeAcp => "acp",
            HarnessTransport::CliJsonl => {
                return Err(Unsupported("unsupported protocol transport"));
            }
        };
        Ok(LaunchRequest {
            program: self.bridge_program.clone(),
            args: vec![
                mode.into(),
                self.native_program
                    .to_str()
                    .ok_or(Unsupported("non-UTF8 executable"))?
                    .into(),
                session.spec.model.name.clone(),
            ],
            initial_input: Some(prompt),
            pty: false,
        })
    }
}
