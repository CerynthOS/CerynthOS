use anyhow::Result;
use axum::{Router, routing::post};
use cerynth_inferenced::{
    AdminOperation, Coordinator, EndpointSpec, InferenceRequest, SessionGrant,
};
use cerynth_resources::pressure::{Sample, now_ms};
use serde_json::json;
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;

fn sample(blocked: bool) -> Sample {
    Sample {
        version: 1,
        observed_at_ms: now_ms(),
        available_memory_bytes: Some(1024_u64.pow(3)),
        memory_some_avg10: Some(0.0),
        memory_full_avg10: Some(if blocked { 6.0 } else { 0.0 }),
        cpu_some_avg10: Some(0.0),
        io_full_avg10: Some(0.0),
    }
}

#[tokio::test]
async fn owner_pressure_queues_before_backend_and_fresh_health_releases_work() -> Result<()> {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let router=Router::new().route("/api/chat",post(move || {let calls=seen.clone();async move {
        calls.fetch_add(1,Ordering::SeqCst);
        format!("{}\n",json!({"model":"fixture","message":{"role":"assistant","content":"4"},"done":true,"eval_count":1}))
    }}));
    let backend = tokio::spawn(async move { axum::serve(listener, router).await });
    let coordinator = Arc::new(Coordinator::new(1));
    coordinator
        .admin(AdminOperation::Configure {
            endpoint: EndpointSpec::Ollama {
                id: "local".into(),
                url: origin,
            },
        })
        .await?;
    let id = Uuid::new_v4();
    let grant = coordinator
        .admin(AdminOperation::Register {
            grant: SessionGrant {
                session_id: id,
                endpoint_id: "local".into(),
                trust_domain: "public-fixture".into(),
                allowed_models: BTreeSet::from(["fixture".into()]),
                concurrency_limits: vec![(id.to_string(), 1)],
                max_priority: 0,
                allow_cloud: false,
                disable_thinking: false,
            },
        })
        .await?;
    coordinator
        .admin(AdminOperation::Pressure {
            sample: sample(true),
        })
        .await?;
    let request = InferenceRequest {
        session_id: id,
        token: grant["token"].as_str().unwrap().into(),
        model: "fixture".into(),
        prompt: "public fixture".into(),
        max_output_tokens: 4,
        priority: 0,
    };
    let worker = coordinator.clone();
    let pending = tokio::spawn(async move { worker.infer(request).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if coordinator.admin(AdminOperation::Health).await.unwrap()["queued_requests"] == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!pending.is_finished());
    let mut stale = sample(false);
    stale.observed_at_ms = stale.observed_at_ms.saturating_sub(6000);
    assert!(
        coordinator
            .admin(AdminOperation::Pressure { sample: stale })
            .await
            .is_err()
    );
    assert_eq!(
        coordinator.admin(AdminOperation::Health).await?["pressure_admission_blocked"],
        true
    );
    coordinator
        .admin(AdminOperation::Pressure {
            sample: sample(false),
        })
        .await?;
    let result = tokio::time::timeout(Duration::from_secs(2), pending).await???;
    assert_eq!(result["response"]["message"]["content"], "4");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        serde_json::from_value::<InferenceRequest>(
            json!({"session_id":id,"token":"fake","model":"fixture",
        "prompt":"public fixture","max_output_tokens":4,"priority":0,"pressure":{"blocked":false}})
        )
        .is_err()
    );
    backend.abort();
    Ok(())
}
