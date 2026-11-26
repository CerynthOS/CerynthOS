use anyhow::Result;
use cerynth_inferenced::{
    AdminOperation, Coordinator, EndpointSpec, InferenceRequest, SessionGrant,
};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Semaphore,
};
use uuid::Uuid;

async fn endpoint() -> Result<(
    String,
    Arc<Semaphore>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let release = Arc::new(Semaphore::new(0));
    let requests = Arc::new(AtomicUsize::new(0));
    let gate = release.clone();
    let counter = requests.clone();
    let task = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (mut stream, _) = result.unwrap();
                    let gate = gate.clone(); let counter = counter.clone();
                    tasks.spawn(async move {
                        let mut bytes = vec![0; 65536];
                        let count = stream.read(&mut bytes).await.unwrap(); assert!(count > 0);
                        counter.fetch_add(1, Ordering::SeqCst);
                        gate.acquire().await.unwrap().forget();
                        let body = concat!(
                            "{\"model\":\"fixture\",\"message\":{\"role\":\"assistant\",\"content\":\"hello\"},\"done\":false}\n",
                            "{\"model\":\"fixture\",\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"eval_count\":1,\"eval_duration\":1000000}\n");
                        let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        let _ = stream.write_all(response.as_bytes()).await;
                    });
                }
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
    });
    Ok((url, release, requests, task))
}

fn grant(id: Uuid, domain: &str) -> SessionGrant {
    SessionGrant {
        session_id: id,
        endpoint_id: "local".into(),
        trust_domain: domain.into(),
        allowed_models: BTreeSet::from(["fixture".into()]),
        concurrency_limits: vec![(format!("session:{id}"), 1), ("project:fixture".into(), 1)],
        max_priority: 10,
        allow_cloud: false,
        disable_thinking: false,
    }
}

fn request(id: Uuid, token: &str) -> InferenceRequest {
    InferenceRequest {
        session_id: id,
        token: token.into(),
        model: "fixture".into(),
        prompt: "private prompt".into(),
        max_output_tokens: 16,
        priority: 0,
    }
}

#[tokio::test]
async fn authenticated_queueing_revocation_and_cache_domain_isolation() -> Result<()> {
    let (url, release, requests, backend) = endpoint().await?;
    let coordinator = Arc::new(Coordinator::new(1));
    coordinator
        .admin(AdminOperation::Configure {
            endpoint: EndpointSpec::Ollama {
                id: "local".into(),
                url: url.clone(),
            },
        })
        .await?;
    let id = Uuid::new_v4();
    let registered = coordinator
        .admin(AdminOperation::Register {
            grant: grant(id, "tenant-a"),
        })
        .await?;
    let token = registered["token"].as_str().unwrap();
    assert!(coordinator.infer(request(id, "wrong")).await.is_err());
    let mut forbidden = request(id, token);
    forbidden.model = "other-model".into();
    assert!(coordinator.infer(forbidden).await.is_err());
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    let mut other = grant(Uuid::new_v4(), "tenant-b");
    assert!(
        coordinator
            .admin(AdminOperation::Register {
                grant: other.clone()
            })
            .await
            .is_err()
    );
    // Aliasing the same backend origin does not bypass cache-domain checks.
    coordinator
        .admin(AdminOperation::Configure {
            endpoint: EndpointSpec::Ollama {
                id: "alias".into(),
                url,
            },
        })
        .await?;
    other.endpoint_id = "alias".into();
    assert!(
        coordinator
            .admin(AdminOperation::Register { grant: other })
            .await
            .is_err()
    );
    let running = {
        let c = coordinator.clone();
        let r = request(id, token);
        tokio::spawn(async move { c.infer(r).await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while requests.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let id2 = Uuid::new_v4();
    let registered2 = coordinator
        .admin(AdminOperation::Register {
            grant: grant(id2, "tenant-a"),
        })
        .await?;
    let waiting = {
        let c = coordinator.clone();
        let r = request(id2, registered2["token"].as_str().unwrap());
        tokio::spawn(async move { c.infer(r).await })
    };
    tokio::time::sleep(Duration::from_millis(10)).await;
    coordinator
        .admin(AdminOperation::Revoke { session_id: id2 })
        .await?;
    assert!(waiting.await?.is_err());
    release.add_permits(1);
    let result = running.await??;
    assert_eq!(result["response"]["message"]["content"], "hello");
    assert!(result["metrics"]["ttft_ms"].is_number());
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    let metrics = coordinator.admin(AdminOperation::Metrics).await?;
    assert!(!metrics.to_string().contains("private prompt"));
    assert!(!metrics.to_string().contains(token));
    assert!(metrics["p95_ms"].is_number());
    coordinator
        .admin(AdminOperation::Revoke { session_id: id })
        .await?;
    assert!(
        coordinator
            .admin(AdminOperation::Register {
                grant: grant(Uuid::new_v4(), "tenant-b")
            })
            .await
            .is_err()
    );
    backend.abort();
    Ok(())
}
