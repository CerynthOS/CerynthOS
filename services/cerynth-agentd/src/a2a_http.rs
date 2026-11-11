//! Explicitly started A2A 0.3 task subset. No automatic execution or full-conformance claim.
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cerynth_agent_api::Operation;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions, io::Write, net::SocketAddr, os::unix::fs::OpenOptionsExt, path::PathBuf,
    sync::Arc, time::Duration,
};
use tokio::sync::Semaphore;
use uuid::Uuid;

struct Gateway {
    directory: PathBuf,
    sponsor: Uuid,
    token_hash: [u8; 32],
    card: Value,
    requests: Semaphore,
}
struct CredentialFile(PathBuf);
impl Drop for CredentialFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// # Errors
/// Rejects non-loopback exposure, unsafe credential storage, absent sponsor authority,
/// or a failed listener. Each start rotates the bounded gateway credential.
pub async fn serve(
    directory: PathBuf,
    listen: SocketAddr,
    sponsor: Uuid,
    credential_file: PathBuf,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    ensure!(
        listen.ip().is_loopback(),
        "A2A prototype requires numeric loopback"
    );
    ensure!(
        credential_file.is_absolute(),
        "credential file must be absolute"
    );
    let parent = credential_file
        .parent()
        .context("credential parent missing")?;
    crate::registry::private_directory(parent)?;
    ensure!(
        parent
            .canonicalize()?
            .starts_with(directory.canonicalize()?),
        "gateway credentials must remain inside the private agentd control directory"
    );
    let status = crate::call(&directory, Operation::Status { id: sponsor }).await?;
    let sponsor_record: cerynth_agent_api::Session = serde_json::from_value(status)?;
    ensure!(
        !sponsor_record.state.is_terminal()
            && sponsor_record
                .spec
                .runtime
                .capabilities
                .contains("task.delegate"),
        "live sponsor with task.delegate required"
    );
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&credential_file)?;
    let _credential = CredentialFile(credential_file.clone());
    output.write_all(token.as_bytes())?;
    output.sync_all()?;
    let gateway = Arc::new(Gateway {
        directory,
        sponsor,
        token_hash: Sha256::digest(token.as_bytes()).into(),
        requests: Semaphore::new(16),
        card: json!({
            "protocolVersion":"0.3.0", "name":"Cerynth supervised task gateway",
            "description":"Experimental text task assignment/get/cancel subset; owner starts execution separately",
            "url":format!("{origin}/a2a"), "version":"0.1.0", "preferredTransport":"JSONRPC",
            "capabilities":{"streaming":false,"pushNotifications":false,"stateTransitionHistory":false},
            "defaultInputModes":["text/plain"],"defaultOutputModes":["application/json"],
            "securitySchemes":{"sponsorToken":{"type":"http","scheme":"bearer"}},"security":[{"sponsorToken":[]}],
            "skills":[{"id":"supervised-task","name":"Supervised task drafts","description":"Submit bounded text task drafts under one sponsor, inspect or cancel; no automatic execution","tags":["task","supervised"]}]
        }),
    });
    let router = Router::new()
        .route("/.well-known/agent-card.json", get(card))
        .route("/a2a", post(request))
        .layer(DefaultBodyLimit::max(cerynth_agent_api::MAX_FRAME_BYTES))
        .with_state(gateway);
    println!(
        "{}",
        json!({"listening":origin,"credential_file":credential_file,"automatic_execution":false,"subset":true})
    );
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

async fn card(State(gateway): State<Arc<Gateway>>) -> Json<Value> {
    Json(gateway.card.clone())
}

fn error(id: &Value, code: i32, message: &'static str) -> Response {
    Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})).into_response()
}

async fn request(
    State(gateway): State<Arc<Gateway>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    let supplied = headers
        .get("authorization")
        .and_then(|s| s.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| s.len() == 64);
    let authenticated = supplied.is_some_and(|s| {
        Sha256::digest(s.as_bytes())
            .iter()
            .zip(gateway.token_hash)
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
    });
    if !authenticated {
        return (
            StatusCode::UNAUTHORIZED,
            [("www-authenticate", "Bearer")],
            "gateway credential required",
        )
            .into_response();
    }
    if !headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|mime| mime.trim() == "application/json")
        })
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let Ok(_permit) = gateway.requests.try_acquire() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let Ok(body) = serde_json::from_slice::<Value>(&bytes) else {
        return error(&Value::Null, -32700, "Invalid JSON");
    };
    let id = &body["id"];
    if body["jsonrpc"] != "2.0"
        || !(id.is_number() || id.as_str().is_some_and(|s| s.len() <= 256))
        || !body.as_object().is_some_and(|o| {
            o.keys()
                .all(|k| matches!(k.as_str(), "jsonrpc" | "id" | "method" | "params"))
        })
    {
        return error(&Value::Null, -32600, "Invalid JSON-RPC request");
    }
    let allowed = match body["method"].as_str() {
        Some("message/send") => {
            body["params"]
                .as_object()
                .is_some_and(|o| o.len() == 1 && o.contains_key("message"))
                && body["params"]["message"].as_object().is_some_and(|o| {
                    o.keys()
                        .all(|k| matches!(k.as_str(), "kind" | "role" | "messageId" | "parts"))
                })
        }
        Some("tasks/get" | "tasks/cancel") => body["params"]
            .as_object()
            .is_some_and(|o| o.len() == 1 && o.contains_key("id")),
        _ => return error(id, -32601, "Method unsupported by this task subset"),
    };
    if !allowed {
        return error(id, -32602, "Unsupported task parameters");
    }
    let operation = Operation::A2a {
        sponsor: gateway.sponsor,
        request: body.clone(),
    };
    match tokio::time::timeout(
        Duration::from_secs(10),
        crate::call(&gateway.directory, operation),
    )
    .await
    {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(_)) => error(id, -32001, "Task unavailable or outside sponsor authority"),
        Err(_) => error(id, -32000, "Task outcome unknown; query before retrying"),
    }
}
