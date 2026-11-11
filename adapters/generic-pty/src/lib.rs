//! PTY transport. The supervisor owns policy, process accounting, and cleanup.

use cerynth_agent_api::{AdapterCapability, AdapterDescriptor, HarnessAdapter, HarnessKind};

pub struct GenericPtyAdapter;

#[async_trait::async_trait]
impl HarnessAdapter for GenericPtyAdapter {
    fn descriptor(&self) -> AdapterDescriptor {
        AdapterDescriptor {
            kind: HarnessKind::GenericPty,
            name: "generic-pty".into(),
            implementation: "portable-pty".into(),
            capabilities: vec![
                AdapterCapability::Launch,
                AdapterCapability::Input,
                AdapterCapability::Cancellation,
            ],
        }
    }

    async fn check_installation(&self) -> Result<bool, cerynth_agent_api::Unsupported> {
        Ok(true)
    }

    async fn prepare_launch(
        &self,
        session: &cerynth_agent_api::Session,
        input: cerynth_agent_api::AdapterInput,
    ) -> Result<cerynth_agent_api::LaunchRequest, cerynth_agent_api::Unsupported> {
        if session.spec.harness != HarnessKind::GenericPty {
            return Err(cerynth_agent_api::Unsupported(
                "generic transport requires a generic_pty session",
            ));
        }
        match input {
            cerynth_agent_api::AdapterInput::Command(launch) => Ok(launch),
            cerynth_agent_api::AdapterInput::Prompt(_) => Err(cerynth_agent_api::Unsupported(
                "generic transport requires explicit program/argv",
            )),
        }
    }
}

pub struct PtyProcess {
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    pub reader: Box<dyn std::io::Read + Send>,
    pub writer: Box<dyn std::io::Write + Send>,
}

/// Spawn only a supervisor-prepared command with an explicit environment.
///
/// # Errors
/// Returns an error if PTY creation or process execution fails.
pub fn spawn(
    program: &std::path::Path,
    args: &[String],
    workspace: &std::path::Path,
    environment: &[(String, String)],
) -> anyhow::Result<PtyProcess> {
    let pair = portable_pty::native_pty_system().openpty(portable_pty::PtySize {
        rows: 30,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut command = portable_pty::CommandBuilder::new(program);
    command.args(args);
    command.cwd(workspace);
    command.env_clear();
    for (key, value) in environment {
        command.env(key, value);
    }
    // Finish fallible IO setup before a child exists, so setup errors cannot
    // abandon a live process outside the supervisor's ownership.
    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    let child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    Ok(PtyProcess {
        master: pair.master,
        child,
        reader,
        writer,
    })
}
