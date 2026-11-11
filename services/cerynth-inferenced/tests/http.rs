use anyhow::Result;
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::HeaderMap,
    response::IntoResponse,
    routing::post,
};
use cerynth_inferenced::{AdminOperation, Coordinator, EndpointSpec, SessionGrant};
use serde_json::{Value, json};
use std::{collections::BTreeSet, convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::{Mutex, Semaphore};
use uuid::Uuid;

struct Fixture {
    gate: Semaphore,
    bodies: Mutex<Vec<Value>>,
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            gate: Semaphore::new(0),
            bodies: Mutex::new(Vec::new()),
        }
    }
}

async fn backend(
    State(state): State<Arc<Fixture>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    assert!(
        headers.get("authorization").is_none(),
        "Cerynth session tokens must never be forwarded to Ollama"
    );
    state.bodies.lock().await.push(body);
    let stream = futures_util::stream::unfold((0, state), |(step, state)| async move {
        let bytes=match step {
            0=>Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"call_fixture\",\"function\":{\"name\":\"edit\",\"arguments\":\"{}\"}}]}}]}\n\n"),
            1=>{state.gate.acquire().await.unwrap().forget();Bytes::from_static(b"data: {\"usage\":{\"completion_tokens\":1},\"choices\":[]}\n\ndata: [DONE]\n\n")},
            _=>return None,
        };
        Some((Ok::<_, Infallible>(bytes), (step + 1, state)))
    });
    (
        [("content-type", "text/event-stream")],
        Body::from_stream(stream),
    )
}

async fn register(coordinator: &Coordinator, id: Uuid) -> Result<String> {
    let response = coordinator
        .admin(AdminOperation::Register {
            grant: SessionGrant {
                session_id: id,
                endpoint_id: "local".into(),
                trust_domain: "owner".into(),
                allowed_models: BTreeSet::from(["fixture".into()]),
                concurrency_limits: vec![(id.to_string(), 1), ("project".into(), 1)],
                max_priority: 0,
                allow_cloud: false,
                disable_thinking: true,
            },
        })
        .await?;
    Ok(response["token"].as_str().unwrap().into())
}

// One full HTTP exchange checks auth, transparent tool payloads, streaming leases and revocation.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn http_preserves_tools_authenticates_every_request_and_holds_queue_until_stream_ends()
-> Result<()> {
    let fixture = Arc::new(Fixture::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let router = Router::new()
        .route("/v1/chat/completions", post(backend))
        .with_state(fixture.clone());
    let upstream = tokio::spawn(async move { axum::serve(listener, router).await });
    let coordinator = Arc::new(Coordinator::new(1));
    coordinator
        .admin(AdminOperation::Configure {
            endpoint: EndpointSpec::Ollama {
                id: "local".into(),
                url: origin,
            },
        })
        .await?;
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let first_token = register(&coordinator, first).await?;
    let second_token = register(&coordinator, second).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let router = cerynth_inferenced::proxy::router(coordinator.clone());
    let proxy = tokio::spawn(async move { axum::serve(listener, router).await });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?;
    let body = json!({"model":"fixture","messages":[{"role":"user","content":"private fixture context"}],
        "stream":true,"tools":[{"type":"function","function":{"name":"edit","parameters":{"type":"object"}}}],"max_tokens":16});
    let route = format!("{endpoint}/v1/chat/completions");
    for field in ["kv_transfer_params", "cache_salt", "session_id", "priority"] {
        let mut engine_control = body.clone();
        engine_control[field] = json!({"remote_host":"forbidden.invalid"});
        assert_eq!(
            client
                .post(&route)
                .bearer_auth(&first_token)
                .json(&engine_control)
                .send()
                .await?
                .status(),
            400
        );
    }
    let mut oversized = body.clone();
    oversized["max_completion_tokens"] = json!(999_999);
    assert_eq!(
        client
            .post(&route)
            .bearer_auth(&first_token)
            .json(&oversized)
            .send()
            .await?
            .status(),
        400
    );
    assert_eq!(client.post(&route).json(&body).send().await?.status(), 401);
    assert_eq!(
        client
            .post(&route)
            .bearer_auth("invalid")
            .json(&body)
            .send()
            .await?
            .status(),
        401
    );
    let mut forbidden = body.clone();
    forbidden["model"] = json!("forbidden");
    assert_eq!(
        client
            .post(&route)
            .bearer_auth(&first_token)
            .json(&forbidden)
            .send()
            .await?
            .status(),
        403
    );
    let mut stateful = body.clone();
    stateful["previous_response_id"] = json!("other-session-context");
    assert_eq!(
        client
            .post(&route)
            .bearer_auth(&first_token)
            .json(&stateful)
            .send()
            .await?
            .status(),
        400
    );
    let mut response = client
        .post(&route)
        .bearer_auth(&first_token)
        .json(&body)
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-cerynth-session"], first.to_string());
    assert!(response.chunk().await?.is_some());
    let second_request = {
        let client = client.clone();
        let route = route.clone();
        let body = body.clone();
        tokio::spawn(async move {
            client
                .post(route)
                .bearer_auth(second_token)
                .json(&body)
                .send()
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let health = coordinator.admin(AdminOperation::Health).await.unwrap();
            if health["queued_requests"] == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert_eq!(fixture.bodies.lock().await.len(), 1);
    coordinator
        .admin(AdminOperation::Revoke { session_id: first })
        .await?;
    assert!(response.text().await.is_err());
    let response = second_request.await??;
    fixture.gate.add_permits(2);
    assert!(response.text().await?.contains("call_fixture"));
    let bodies = fixture.bodies.lock().await;
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["tools"], body["tools"]);
    assert_eq!(bodies[0]["reasoning_effort"], "none");
    drop(bodies);
    let metrics = coordinator.admin(AdminOperation::Metrics).await?;
    assert!(!metrics.to_string().contains("private fixture context"));
    assert!(!metrics.to_string().contains(&first_token));
    assert_eq!(metrics["samples"][0]["session_id"], second.to_string());
    assert_eq!(
        client
            .get(format!("{endpoint}/control"))
            .send()
            .await?
            .status(),
        404
    );
    proxy.abort();
    upstream.abort();
    Ok(())
}
