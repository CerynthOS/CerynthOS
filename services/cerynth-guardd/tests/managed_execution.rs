//! Process-level runtime contract test, including the real sandbox.
use cerynth_guardd::{
    client::Client,
    protocol::{Registration, Request, Response},
};
use cerynth_security::{CapabilitySet, GuardSession, SessionMode};
use std::{
    collections::BTreeMap,
    process::{Child, Command},
    time::Duration,
};
use uuid::Uuid;
struct Stop(Child);
impl Drop for Stop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[tokio::test]
#[ignore = "requires bubblewrap, user namespaces, Landlock and prebuilt cerynth-guard-exec"]
async fn managed_execution_and_cleanup() {
    let tmp = tempfile::tempdir().unwrap();
    let run = tmp.path().join("run");
    let state = tmp.path().join("state");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let guard_exec =
        std::env::var_os("GUARD_EXEC").expect("set GUARD_EXEC to the built confinement binary");
    let backend_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_url = format!("http://{}/", backend_listener.local_addr().unwrap());
    let backend = tokio::spawn(async move {
        axum::serve(
            backend_listener,
            axum::Router::new().fallback(|| async { "model-ok" }),
        )
        .await
        .unwrap();
    });
    let config = tmp.path().join("guard.toml");
    std::fs::write(
        &config,
        format!(
            "runtime_dir = {:?}\nstate_dir = {:?}\nguard_exec = {:?}\n[endpoints.local]\nbase_url = {:?}\n",
            run, state, guard_exec, backend_url
        ),
    )
    .unwrap();
    let _daemon = Stop(
        Command::new(env!("CARGO_BIN_EXE_cerynth-guardd"))
            .arg("serve")
            .arg(&config)
            .spawn()
            .unwrap(),
    );
    let client = Client::new(run.join("guard.sock"));
    let mut ready = false;
    for _ in 0..100 {
        if matches!(
            client
                .request(&Request::Status, Duration::from_secs(1))
                .await,
            Ok(Response::Status { .. })
        ) {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ready, "daemon did not become ready");
    let id = Uuid::new_v4();
    let registration = Registration {
        session: GuardSession {
            session_id: id,
            parent_session_id: None,
            mode: SessionMode::Managed,
            workspace: workspace.clone(),
            capabilities: CapabilitySet(vec![cerynth_security::Capability::new(
                cerynth_security::CapabilityKind::InferenceUse,
                "local",
                cerynth_security::Mode::Allow,
            )]),
            harness: Some("contract-test".into()),
        },
        remotes: BTreeMap::new(),
    };
    assert!(matches!(
        client
            .request(
                &Request::RegisterSession { registration },
                Duration::from_secs(2)
            )
            .await
            .unwrap(),
        Response::Ok
    ));
    let response = client.request(&Request::LaunchProfile { session_id:id, cmd:vec!["sh".into(), "-c".into(), "printf verified > /workspace/result; test ! -e /run/cerynth; test ! -e /dev/dri/renderD128; python3 -c \"import urllib.request; assert urllib.request.urlopen('http://127.0.0.1:8099/local/v1/chat/completions', timeout=10).read() == b'model-ok'\"".into()] }, Duration::from_secs(2)).await.unwrap();
    let Response::Launch { argv } = response else {
        panic!("expected launch response");
    };
    let output = tokio::process::Command::new(&argv[0])
        .args(&argv[1..])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("result")).unwrap(),
        "verified"
    );
    let session_client = Client::new(run.join("sessions").join(format!("{id}.sock")));
    assert!(matches!(
        session_client
            .request(&Request::PendingApprovals, Duration::from_secs(2))
            .await
            .unwrap(),
        Response::Error { .. }
    ));
    assert!(matches!(
        client
            .request(
                &Request::EndSession { session_id: id },
                Duration::from_secs(2)
            )
            .await
            .unwrap(),
        Response::Ok
    ));
    assert!(!session_client.socket().exists());
    assert!(matches!(
        client
            .request(&Request::VerifyAudit, Duration::from_secs(2))
            .await
            .unwrap(),
        Response::Audit { .. }
    ));
    backend.abort();
}
