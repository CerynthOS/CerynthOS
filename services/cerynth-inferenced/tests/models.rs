use anyhow::Result;
use axum::{
    Json, Router,
    routing::{get, post},
};
use cerynth_inferenced::{AdminOperation, Coordinator, EndpointSpec};
use serde_json::json;

#[tokio::test]
async fn discovery_is_bounded_local_read_only_and_registry_normalizes_backend_metadata()
-> Result<()> {
    let fixture=Router::new()
        .route("/api/tags",get(async ||Json(json!({"models":[{"name":"fixture","size":100,"digest":"digest",
            "details":{"family":"qwen3","quantization_level":"Q4_K_M"}}]}))))
        .route("/api/ps",get(async ||Json(json!({"models":[{"name":"fixture","size":300,"size_vram":200,"context_length":4096}]}))))
        .route("/api/show",post(async ||Json(json!({"model_info":{"qwen3.context_length":32768},
            "capabilities":["completion","tools"],"license":"unstructured license text"}))));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, fixture).await });
    let coordinator = Coordinator::new(1);
    let discovered = coordinator
        .admin(AdminOperation::Discover {
            candidates: vec![origin.clone(), "http://127.0.0.1:1".into()],
        })
        .await?;
    assert_eq!(discovered["registered"], false);
    assert_eq!(
        discovered["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["reachable"] == true)
            .count(),
        1
    );
    let health = coordinator.admin(AdminOperation::Health).await?;
    assert_eq!(health["endpoints"], json!([]));
    assert!(
        coordinator
            .admin(AdminOperation::Discover {
                candidates: vec!["https://example.invalid".into()]
            })
            .await
            .is_err()
    );
    assert!(
        coordinator
            .admin(AdminOperation::Discover {
                candidates: vec![origin.clone(); 9]
            })
            .await
            .is_err()
    );
    coordinator
        .admin(AdminOperation::Configure {
            endpoint: EndpointSpec::Ollama {
                id: "local".into(),
                url: origin,
            },
        })
        .await?;
    let registry = coordinator
        .admin(AdminOperation::Registry {
            endpoint_id: "local".into(),
            model: Some("fixture".into()),
        })
        .await?;
    let model = &registry["models"][0];
    assert_eq!(model["quantization"], "Q4_K_M");
    assert_eq!(model["architecture_context_limit"], 32768);
    assert_eq!(model["configured_context_length"], 4096);
    assert_eq!(model["allocated_vram_bytes"], 200);
    assert_eq!(model["weight_bytes"], 100);
    assert_eq!(model["loaded"], true);
    assert!(model["device"].is_null());
    assert!(model.get("license").is_none());
    assert!(
        !coordinator.admin(AdminOperation::CachedModels).await?["models"]["records"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    server.abort();
    assert!(
        coordinator
            .admin(AdminOperation::Registry {
                endpoint_id: "local".into(),
                model: None
            })
            .await
            .is_err()
    );
    assert!(
        !coordinator.admin(AdminOperation::CachedModels).await?["models"]["records"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    Ok(())
}
