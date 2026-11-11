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
