//! Token-authenticated OpenAI-compatible data plane. No administrative HTTP routes.
use crate::{Backend, Coordinator, MetricSample, SessionGrant};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cerynth_model_api::{InferenceMetrics, admission::AdmissionPermit};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{OwnedRwLockReadGuard, watch};
use uuid::Uuid;

type Rejection = (StatusCode, Json<Value>);
fn reject(status: StatusCode, message: &'static str) -> Rejection {
    (
        status,
        Json(json!({"error":{"message":message,"type":"cerynth_inference_error"}})),
    )
}

fn residency_unavailable(_: anyhow::Error) -> Rejection {
    reject(
        StatusCode::SERVICE_UNAVAILABLE,
        "model residency unavailable",
    )
}

/// Only chat and stateless Responses routes are exposed, both requiring a grant.
pub fn router(coordinator: Arc<Coordinator>) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/v1/responses", post(responses))
        .route("/v1/models", get(models))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .with_state(coordinator)
}

async fn chat(
    State(coordinator): State<Arc<Coordinator>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, Rejection> {
    forward(coordinator, headers, body, "chat/completions").await
}
async fn responses(
    State(coordinator): State<Arc<Coordinator>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, Rejection> {
    forward(coordinator, headers, body, "responses").await
}

async fn authorize(
    coordinator: &Coordinator,
    headers: &HeaderMap,
    model: Option<&str>,
) -> Result<(SessionGrant, Backend, watch::Receiver<bool>), Rejection> {
    let token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .filter(|token| token.len() == 64)
        .ok_or_else(|| reject(StatusCode::UNAUTHORIZED, "session credential required"))?;
    let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let state = coordinator.state.lock().await;
    let grant = state
        .grants
        .values()
        .find(|grant| {
            hash.iter()
                .zip(grant.token_hash)
                .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
                == 0
        })
        .ok_or_else(|| reject(StatusCode::UNAUTHORIZED, "invalid session credential"))?;
    if model.is_some_and(|model| !grant.spec.allowed_models.contains(model)) {
        return Err(reject(StatusCode::FORBIDDEN, "model is not authorized"));
    }
    let backend = match state.endpoints[&grant.spec.endpoint_id].1.clone() {
        Backend::Compatible(client) => Backend::Compatible(
            client.for_cache_scope(format!("{:x}", Sha256::digest(grant.token_hash))),
        ),
        backend @ Backend::Ollama(_) => backend,
    };
    Ok((grant.spec.clone(), backend, grant.revoked.subscribe()))
}

async fn forward(
    coordinator: Arc<Coordinator>,
    headers: HeaderMap,
    mut body: Value,
    route: &'static str,
) -> Result<Response, Rejection> {
    let model = requested_model(&body)?;
    let (grant, backend, mut revoked) = authorize(&coordinator, &headers, Some(&model)).await?;
    enforce_cache_policy(coordinator.privacy, &body)?;
    let streaming = enforce_request_policy(&mut body, route, &grant)?;
    let priority = headers
        .get("x-cerynth-priority")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.parse::<i32>().ok())
        .unwrap_or(0);
    if priority > grant.max_priority || priority < -100 {
        return Err(reject(StatusCode::FORBIDDEN, "priority is not authorized"));
    }
    let session_id = grant.session_id;
    let endpoint_id = grant.endpoint_id.clone();
    let trace_id = Uuid::new_v4();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let work = async {
        let request_slot = coordinator
            .request_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                reject(
                    StatusCode::TOO_MANY_REQUESTS,
                    "inference request admission is full",
                )
            })?;
        let waiting = Instant::now();
        let lifecycle = coordinator.lifecycle.clone().read_owned().await;
        let residency = coordinator
            .residency_permit(&endpoint_id)
            .await
            .map_err(residency_unavailable)?;
        let permit = coordinator
            .queue
            .acquire(priority, grant.concurrency_limits)
            .await
            .map_err(|_| {
                reject(
                    StatusCode::TOO_MANY_REQUESTS,
                    "inference admission unavailable",
                )
            })?;
        let start = Instant::now();
        let queue_ms = waiting.elapsed().as_secs_f64() * 1000.0;
        let response = match backend {
            Backend::Ollama(client) => client.proxy(route, body).await,
            Backend::Compatible(client) => client.proxy(route, body).await,
        }
        .map_err(|_| reject(StatusCode::BAD_GATEWAY, "inference backend unavailable"))?;
        if !response.status().is_success() {
            return Err(reject(
                StatusCode::BAD_GATEWAY,
                "inference backend rejected request",
            ));
        }
        Ok::<_, Rejection>((
            permit,
            start,
            response,
            lifecycle,
            residency,
            request_slot,
            queue_ms,
        ))
    };
    let (permit, start, response, lifecycle, residency, request_slot, queue_ms) = tokio::select! {
        biased;
        _=revoked.changed()=>return Err(reject(StatusCode::FORBIDDEN,"session grant revoked")),
        result=tokio::time::timeout_at(deadline,work)=>result
            .map_err(|_|reject(StatusCode::GATEWAY_TIMEOUT,"inference admission deadline exceeded"))??,
    };
    let state = ForwardState {
        _request_slot: request_slot,
        response,
        _permit: permit,
        _lifecycle: lifecycle,
        _residency: residency,
        revoked,
        coordinator,
        start,
        queue_ms,
        first_content_ms: None,
        generated_tokens: None,
        pending: Vec::new(),
        total: 0,
        streaming,
        finished: false,
        session_id,
        endpoint_id,
        model,
        trace_id,
        deadline,
    };
    Ok(stream_response(state))
}

fn requested_model(body: &Value) -> Result<String, Rejection> {
    body["model"]
        .as_str()
        .filter(|m| !m.is_empty() && m.len() <= 256)
        .map(str::to_owned)
        .ok_or_else(|| reject(StatusCode::BAD_REQUEST, "model is required"))
}

fn enforce_cache_policy(policy: crate::privacy::Policy, body: &Value) -> Result<(), Rejection> {
    if policy == crate::privacy::Policy::ManagedProcess
        && [
            "cache_salt",
            "cache_prompt",
            "kv_transfer_params",
            "prompt_cache_key",
            "cache_id",
            "cache_namespace",
        ]
        .iter()
        .any(|key| body.get(*key).is_some())
    {
        return Err(reject(
            StatusCode::BAD_REQUEST,
            "client cache control is forbidden in managed-process privacy mode",
        ));
    }
    Ok(())
}

fn stream_response(state: ForwardState) -> Response {
    let content_type = if state.streaming {
        "text/event-stream"
    } else {
        "application/json"
    };
    let session_id = state.session_id;
    let trace_id = state.trace_id;
    let stream = futures_util::stream::try_unfold(state, |mut state| async move {
        let chunk = tokio::select! {
            biased;
            _=state.revoked.changed()=>return Err(std::io::Error::other("inference grant revoked")),
            ()=tokio::time::sleep_until(state.deadline)=>return Err(std::io::Error::other("inference stream deadline exceeded")),
            result=state.response.chunk()=>result.map_err(|_|std::io::Error::other("inference stream failed"))?,
        };
        let Some(chunk) = chunk else {
            state.finish_metrics().await;
            return Ok(None);
        };
        state.total += chunk.len();
        if state.total > 4 * 1024 * 1024 {
            return Err(std::io::Error::other("inference output exceeds limit"));
        }
        state.observe(&chunk);
        Ok::<_, std::io::Error>(Some((chunk, state)))
    });
    (
        [
            ("content-type", content_type.to_string()),
            ("cache-control", "no-store".into()),
            ("x-cerynth-session", session_id.to_string()),
            ("x-cerynth-trace", trace_id.to_string()),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

fn enforce_request_policy(
    body: &mut Value,
    route: &str,
    grant: &SessionGrant,
) -> Result<bool, Rejection> {
    for forbidden in [
        "previous_response_id",
        "conversation",
        "api_key",
        "base_url",
        "headers",
        "kv_transfer_params",
        "remote_kv_source",
        "session_params",
        "session_id",
        "cache_salt",
        "cache_control",
        "priority",
        "lora_request",
    ] {
        if body.get(forbidden).is_some_and(|value| !value.is_null()) {
            return Err(reject(
                StatusCode::BAD_REQUEST,
                "stateful context and routing overrides are unsupported",
            ));
        }
    }
    if body["n"].as_u64().is_some_and(|n| n != 1) || body["background"] == true {
        return Err(reject(
            StatusCode::BAD_REQUEST,
            "multiple completions and background requests are unsupported",
        ));
    }
    let output_limit = if route == "responses" {
        "max_output_tokens"
    } else {
        "max_tokens"
    };
    if ["max_tokens", "max_output_tokens", "max_completion_tokens"]
        .iter()
        .any(|key| {
            body.get(*key)
                .is_some_and(|limit| limit.as_u64().is_none_or(|n| n == 0 || n > 4096))
        })
    {
        return Err(reject(
            StatusCode::BAD_REQUEST,
            "output token limit must be 1..4096",
        ));
    }
    body[output_limit] = body
        .get(output_limit)
        .cloned()
        .unwrap_or_else(|| json!(4096));
    let streaming = body["stream"] == true;
    if grant.disable_thinking {
        if route == "responses" {
            body["think"] = json!(false);
            body["reasoning"] = json!({"effort":"none"});
        } else {
            body["reasoning_effort"] = json!("none");
        }
    }
    if route == "responses" {
        body["store"] = json!(false);
    }
    if body.get("prompt_cache_key").is_some() {
        body["prompt_cache_key"] = json!(format!("cerynth-{}", grant.session_id));
    }
    Ok(streaming)
}

struct ForwardState {
    _request_slot: tokio::sync::OwnedSemaphorePermit,
    _residency: OwnedRwLockReadGuard<()>,
    response: reqwest::Response,
    _permit: AdmissionPermit,
    _lifecycle: OwnedRwLockReadGuard<()>,
    revoked: watch::Receiver<bool>,
    coordinator: Arc<Coordinator>,
    start: Instant,
    queue_ms: f64,
    first_content_ms: Option<f64>,
    generated_tokens: Option<u64>,
    pending: Vec<u8>,
    total: usize,
    streaming: bool,
    finished: bool,
    session_id: Uuid,
    endpoint_id: String,
    model: String,
    trace_id: Uuid,
    deadline: tokio::time::Instant,
}

impl ForwardState {
    fn observe(&mut self, chunk: &[u8]) {
        if self.pending.len() + chunk.len() > 4 * 1024 * 1024 {
            self.pending.clear();
            return;
        }
        self.pending.extend_from_slice(chunk);
        if self.streaming {
            while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = self.pending.drain(..=end).collect();
                if let Some(data) = line.strip_prefix(b"data: ")
                    && let Ok(value) = serde_json::from_slice::<Value>(data)
                {
                    self.observe_event(&value);
                }
            }
        }
    }

    fn observe_event(&mut self, value: &Value) {
        if value["choices"][0]["delta"]["content"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
            || value["type"] == "response.output_text.delta"
            || value["choices"][0]["delta"]["tool_calls"].is_array()
        {
            self.first_content_ms
                .get_or_insert_with(|| self.start.elapsed().as_secs_f64() * 1000.0);
        }
        self.generated_tokens = value["usage"]["completion_tokens"]
            .as_u64()
            .or_else(|| value["response"]["usage"]["output_tokens"].as_u64())
            .or(self.generated_tokens);
    }

    async fn finish_metrics(&mut self) {
        if !self.streaming
            && let Ok(value) = serde_json::from_slice::<Value>(&self.pending)
        {
            self.generated_tokens = value["usage"]["completion_tokens"]
                .as_u64()
                .or_else(|| value["usage"]["output_tokens"].as_u64());
        }
        let metrics = InferenceMetrics {
            queue_ms: self.queue_ms,
            request_ms: self.start.elapsed().as_secs_f64() * 1000.0,
            ttft_ms: self.first_content_ms,
            generated_tokens: self.generated_tokens,
            tokens_per_second: None,
        };
        let mut state = self.coordinator.state.lock().await;
        if state.metrics.len() == 1024 {
            state.metrics.remove(0);
        }
        state.metrics.push(MetricSample {
            session_id: self.session_id,
            endpoint_id: self.endpoint_id.clone(),
            model: self.model.clone(),
            trace_id: self.trace_id,
            metrics,
        });
        self.finished = true;
    }
}

impl Drop for ForwardState {
    fn drop(&mut self) {
        if !self.finished {
            let coordinator = self.coordinator.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    coordinator.state.lock().await.failures += 1;
                });
            }
        }
    }
}

async fn models(
    State(coordinator): State<Arc<Coordinator>>,
    headers: HeaderMap,
) -> Result<Json<Value>, Rejection> {
    let (grant, _, _) = authorize(&coordinator, &headers, None).await?;
    Ok(Json(
        json!({"object":"list","data":grant.allowed_models.into_iter().map(|model|
        json!({"id":model,"object":"model","owned_by":"cerynth"})).collect::<Vec<_>>()}),
    ))
}
