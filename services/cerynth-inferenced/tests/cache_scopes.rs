use anyhow::Result;
use axum::{Json, Router, extract::State, routing::post};
use cerynth_inferenced::{
    AdminOperation, Coordinator, EndpointSpec, InferenceRequest, SessionGrant,
};
use cerynth_model_api::compatible::{CacheIsolation, CompatibleClient, CompatibleEndpoint};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use uuid::Uuid;

async fn register(coordinator: &Coordinator, id: Uuid) -> Result<String> {
    let receipt = coordinator
        .admin(AdminOperation::Register {
            grant: SessionGrant {
                session_id: id,
                endpoint_id: "local".into(),
                trust_domain: "owner".into(),
                allowed_models: BTreeSet::from(["fixture".into()]),
                concurrency_limits: vec![(id.to_string(), 1)],
                max_priority: 0,
                allow_cloud: false,
                disable_thinking: false,
            },
        })
        .await?;
    Ok(receipt["token"].as_str().unwrap().into())
}

async fn backend(
    State(bodies): State<Arc<Mutex<Vec<Value>>>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    bodies.lock().await.push(body);
    Json(
        json!({"choices":[{"message":{"role":"assistant","content":"4"}}],
        "usage":{"prompt_tokens":4,"completion_tokens":1}}),
    )
}

// Exercise both data planes, forgery rejection and credential rotation against
// an independent HTTP backend that records the actual engine-bound request.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn private_cache_scopes_are_stable_per_grant_distinct_and_not_client_selectable() -> Result<()>
{
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let config = CompatibleEndpoint {
        url: format!("http://{}/v1", listener.local_addr()?),
        cloud_consent: false,
        credential_env: None,
        cache_isolation: CacheIsolation::VllmPerGrantSalt,
    };
    let unscoped = CompatibleClient::new(&config)?;
    assert!(
        unscoped
            .infer("fixture", "private fixture", 4)
            .await
            .is_err()
    );
    let router = Router::new()
        .route("/v1/chat/completions", post(backend))
        .with_state(bodies.clone());
    let upstream = tokio::spawn(async move { axum::serve(listener, router).await });
    let coordinator = Arc::new(Coordinator::new(2));
    coordinator
        .admin(AdminOperation::Configure {
            endpoint: EndpointSpec::Compatible {
                id: "local".into(),
                config,
            },
        })
        .await?;
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let first_token = register(&coordinator, first).await?;
    let second_token = register(&coordinator, second).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let router = cerynth_inferenced::proxy::router(coordinator.clone());
    let proxy = tokio::spawn(async move { axum::serve(listener, router).await });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?;
    let body = json!({"model":"fixture","messages":[{"role":"user","content":"private fixture"}],"stream":false});
    for token in [&first_token, &first_token, &second_token, &second_token] {
        assert!(
            client
                .post(format!("{origin}/v1/chat/completions"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await?
                .status()
                .is_success()
        );
    }
    coordinator
        .infer(InferenceRequest {
            session_id: first,
            token: first_token.clone(),
            model: "fixture".into(),
            prompt: "private fixture".into(),
            max_output_tokens: 4,
            priority: 0,
        })
        .await?;
    let mut forged = body.clone();
    forged["cache_salt"] = json!("a".repeat(64));
    assert_eq!(
        client
            .post(format!("{origin}/v1/chat/completions"))
            .bearer_auth(&first_token)
            .json(&forged)
            .send()
            .await?
            .status(),
        400
    );
    assert_eq!(
        client
            .post(format!("{origin}/v1/responses"))
            .bearer_auth(&first_token)
            .json(&json!({"model":"fixture","input":"private fixture"}))
            .send()
            .await?
            .status(),
        502
    );
    coordinator
        .admin(AdminOperation::Revoke { session_id: first })
        .await?;
    let replacement = register(&coordinator, first).await?;
    assert!(
        client
            .post(format!("{origin}/v1/chat/completions"))
            .bearer_auth(&replacement)
            .json(&body)
            .send()
            .await?
            .status()
            .is_success()
    );
    let records = bodies.lock().await;
    assert_eq!(records.len(), 6);
    let salts: Vec<_> = records
        .iter()
        .map(|body| body["cache_salt"].as_str().unwrap())
        .collect();
    assert_eq!(salts[0], salts[1]);
    assert_eq!(salts[0], salts[4]);
    assert_eq!(salts[2], salts[3]);
    assert_ne!(salts[0], salts[2]);
    assert_ne!(salts[0], salts[5]);
    assert!(
        salts
            .iter()
            .all(|salt| salt.len() == 64 && *salt != first_token && *salt != second_token)
    );
    proxy.abort();
    upstream.abort();
    Ok(())
}
