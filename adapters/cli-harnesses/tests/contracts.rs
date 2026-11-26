use cerynth_adapter_cli_harnesses::CliHarnessAdapter;
use cerynth_agent_api::{
    AdapterCapability, AdapterInput, ExecutionBoundary, ExecutionRecord, HarnessAdapter,
    HarnessKind, LaunchRequest, ModelConfig, ModelMode, RuntimeSpec, Session, SessionSpec,
    SessionState,
};

fn session(kind: HarnessKind) -> Session {
    Session {
        id: uuid::Uuid::nil(),
        owner_uid: 1000,
        spec: SessionSpec {
            name: "contract".into(),
            harness: kind,
            workspace: "/tmp/public-fixture".into(),
            model: ModelConfig {
                name: "org/model:quant".into(),
                mode: ModelMode::Local,
                endpoint_id: None,
                cloud_consent: false,
                disable_thinking: false,
            },
            runtime: RuntimeSpec::default(),
        },
        state: SessionState::Created,
        boundary: ExecutionBoundary::UnenforcedPrototype,
        created_at_ms: 0,
        updated_at_ms: 0,
        events: Vec::new(),
        execution: ExecutionRecord::default(),
    }
}

#[tokio::test]
async fn owner_hermes_profile_keeps_prompt_off_argv_and_does_not_bypass_os_policy() {
    let mut session = session(HarnessKind::Hermes);
    session.spec.runtime.hermes_terminal_profile = true;
    assert!(session.spec.validate().is_ok());
    let adapter = CliHarnessAdapter {
        kind: HarnessKind::Hermes,
        program: "/usr/bin/hermes".into(),
    };
    let launch = adapter
        .prepare_launch(&session, AdapterInput::Prompt("private fixture".into()))
        .await
        .unwrap();
    assert!(
        !launch
            .args
            .iter()
            .any(|a| a == "--safe-mode" || a.contains("private fixture"))
    );
    assert!(launch.args.iter().any(|a| a == "--ignore-rules"));
    assert!(
        launch
            .args
            .windows(2)
            .any(|a| a == ["--toolsets", "terminal"])
    );
    assert!(session.spec.runtime.require_cgroup);
    session.spec.harness = HarnessKind::OpenCode;
    assert!(session.spec.validate().is_err());
}

#[tokio::test]
async fn protocol_plans_keep_native_server_in_the_supervised_launch_and_prompt_off_argv() {
    use cerynth_adapter_cli_harnesses::protocol::OpenCodeProtocolAdapter;
    use cerynth_agent_api::HarnessTransport;
    for transport in [
        HarnessTransport::OpenCodeHttp,
        HarnessTransport::OpenCodeAcp,
    ] {
        let adapter = OpenCodeProtocolAdapter {
            native_program: "/usr/bin/opencode".into(),
            bridge_program: "/usr/lib/cerynth/cerynth-harness-bridge".into(),
            transport,
        };
        let mut session = session(HarnessKind::OpenCode);
        session.spec.runtime.transport = transport;
        let launch = adapter
            .prepare_launch(
                &session,
                AdapterInput::Prompt("private-protocol-prompt".into()),
            )
            .await
            .unwrap();
        assert_eq!(launch.program, adapter.bridge_program);
        assert_eq!(launch.args[1], "/usr/bin/opencode");
        assert_eq!(launch.args[2], "org/model:quant");
        assert!(
            launch
                .args
                .iter()
                .all(|a| !a.contains("private-protocol-prompt"))
        );
        assert_eq!(
            launch.initial_input.as_deref(),
            Some("private-protocol-prompt")
        );
        session.spec.harness = HarnessKind::Hermes;
        assert!(session.spec.validate().is_err());
        assert!(
            adapter
                .prepare_launch(&session, AdapterInput::Prompt("fixture".into()))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn native_adapters_do_not_place_prompts_in_argv_or_advertise_unsupported_control() {
    for kind in [
        HarnessKind::OpenCode,
        HarnessKind::Hermes,
        HarnessKind::Codex,
        HarnessKind::ClaudeCode,
    ] {
        let adapter = CliHarnessAdapter {
            kind,
            program: "/usr/bin/true".into(),
        };
        let session = session(kind);
        let descriptor = adapter.descriptor();
        assert!(descriptor.capabilities.contains(&AdapterCapability::Launch));
        assert!(
            !descriptor
                .capabilities
                .contains(&AdapterCapability::PauseResume)
        );
        assert!(
            !descriptor
                .capabilities
                .contains(&AdapterCapability::ToolApprovals)
        );
        assert!(
            !descriptor
                .capabilities
                .contains(&AdapterCapability::Subagents)
        );
        let launch = adapter
            .prepare_launch(
                &session,
                AdapterInput::Prompt("private prompt fixture".into()),
            )
            .await
            .unwrap();
        assert_eq!(
            launch.initial_input.as_deref(),
            Some("private prompt fixture")
        );
        assert!(
            !launch
                .args
                .iter()
                .any(|arg| arg.contains("private prompt fixture"))
        );
        if kind == HarnessKind::OpenCode {
            assert!(launch.args.contains(&"ollama/org/model:quant".into()));
        }
        assert!(
            adapter
                .prepare_launch(
                    &session,
                    AdapterInput::Command(LaunchRequest {
                        program: "/usr/bin/sh".into(),
                        args: Vec::new(),
                        initial_input: None,
                        pty: false
                    })
                )
                .await
                .is_err()
        );
        let mut mismatched = session;
        mismatched.spec.harness = HarnessKind::InferenceProbe;
        assert!(
            adapter
                .prepare_launch(&mismatched, AdapterInput::Prompt("fixture".into()))
                .await
                .is_err()
        );
    }
    assert_eq!(
        CliHarnessAdapter {
            kind: HarnessKind::Acp,
            program: "/usr/bin/true".into()
        }
        .descriptor()
        .capabilities,
        [] as [AdapterCapability; 0]
    );
}
