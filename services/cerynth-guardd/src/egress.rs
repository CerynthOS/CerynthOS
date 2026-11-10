//! Fixed-endpoint HTTP gateway. Clients use /<endpoint>/<provider-path> on
//! sandbox loopback; they cannot supply an upstream host. HTTP/TLS parsing and
//! streaming use Axum and reqwest. This is not a general CONNECT proxy.
use crate::{state::Guard, vault::Credential};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use cerynth_security::{ActionRequest, CapabilityKind, Verdict};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::net::UnixListener;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub base_url: String,
    #[serde(default)]
    pub credential: Option<String>,
}
impl Endpoint {
    pub fn validate(&self) -> anyhow::Result<()> {
        let u = reqwest::Url::parse(&self.base_url)?;
        anyhow::ensure!(
            u.username().is_empty()
                && u.password().is_none()
                && u.query().is_none()
                && u.fragment().is_none(),
            "endpoint URL cannot contain credentials, query or fragment"
        );
        let local = u
            .host_str()
            .is_some_and(|h| h == "127.0.0.1" || h == "[::1]" || h == "localhost");
        anyhow::ensure!(
            u.scheme() == "https" || (u.scheme() == "http" && local),
            "endpoint requires HTTPS, except explicit loopback HTTP"
        );
        anyhow::ensure!(
            u.host_str().is_some() && u.path() == "/",
            "endpoint must be an origin URL; put provider paths in requests"
        );
        Ok(())
    }
}
#[derive(Clone)]
struct Gateway {
    guard: Arc<Guard>,
    session: Uuid,
    client: reqwest::Client,
}
async fn forward(State(g): State<Gateway>, request: Request) -> Response {
    match perform(g, request).await {
        Ok(r) => r,
        Err((s, m)) => (s, m).into_response(),
    }
}
async fn perform(g: Gateway, request: Request) -> Result<Response, (StatusCode, &'static str)> {
    let deny = || (StatusCode::FORBIDDEN, "endpoint access denied");
    if request.method() != Method::POST && request.method() != Method::GET {
        return Err(deny());
    }
    // Only origin-form requests. CONNECT and absolute-form proxy targets fail.
    if request.uri().authority().is_some() {
        return Err(deny());
    }
    let path = request.uri().path();
    let (name, suffix) = path
        .strip_prefix('/')
        .and_then(|p| p.split_once('/'))
        .ok_or_else(deny)?;
    let endpoint = g.guard.cfg.endpoints.get(name).ok_or_else(deny)?.clone();
    let decision = g
        .guard
        .decide(ActionRequest::new(
            g.session,
            CapabilityKind::InferenceUse,
            name,
            serde_json::json!({}),
        ))
        .await;
    if decision.decision != Verdict::Allow {
        return Err(deny());
    }
    let mut url = reqwest::Url::parse(&endpoint.base_url).map_err(|_| deny())?;
    url.set_path(&format!("/{suffix}"));
    // Inference permission does not grant model deletion, package download,
    // provider administration, or arbitrary web access.
    if !matches!(
        url.path(),
        "/v1/chat/completions"
            | "/v1/completions"
            | "/v1/embeddings"
            | "/v1/models"
            | "/v1/messages"
            | "/v1/responses"
            | "/api/chat"
            | "/api/generate"
            | "/api/embeddings"
            | "/api/tags"
    ) {
        return Err(deny());
    }
    url.set_query(request.uri().query());
    let mut upstream = g.client.request(request.method().clone(), url.clone());
    for name in [
        "content-type",
        "accept",
        "anthropic-version",
        "anthropic-beta",
    ] {
        if let Some(value) = request.headers().get(name) {
            upstream = upstream.header(name, value);
        }
    }
    if let Some(name) = endpoint.credential {
        let decision = g
            .guard
            .decide(ActionRequest::new(
                g.session,
                CapabilityKind::CredentialUse,
                &name,
                serde_json::json!({}),
            ))
            .await;
        if decision.decision != Verdict::Allow {
            return Err(deny());
        }
        match g.guard.vault.get(&name) {
            Some(Credential::HttpHeader {
                header,
                value,
                base_url,
            }) if base_url == &endpoint.base_url => {
                let key = reqwest::header::HeaderName::from_bytes(header.as_bytes())
                    .map_err(|_| deny())?;
                // Credentials cannot alter routing or HTTP framing.
                if key != "authorization" && key != "x-api-key" && key != "api-key" {
                    return Err(deny());
                }
                upstream = upstream.header(key, value);
            }
            _ => return Err(deny()),
        }
    }
    let name = name.to_owned();
    let body = to_bytes(request.into_body(), 8 * 1024 * 1024)
        .await
        .map_err(|_| (StatusCode::PAYLOAD_TOO_LARGE, "request too large"))?;
    g.guard
        .record(
            "guard.egress.started",
            serde_json::json!({"session_id":g.session,"endpoint":name}),
        )
        .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "audit unavailable"))?;
    let reply = upstream
        .body(body)
        .send()
        .await
        .map_err(|_| (StatusCode::BAD_GATEWAY, "upstream unavailable"))?;
    // Never forward redirects or sensitive response headers to the sandbox.
    if reply.status().is_redirection() {
        return Err((StatusCode::BAD_GATEWAY, "upstream redirect blocked"));
    }
    let mut response = Response::builder().status(reply.status());
    if let Some(value) = reply.headers().get("content-type") {
        response = response.header("content-type", value);
    }
    response
        .body(Body::from_stream(reply.bytes_stream()))
        .map_err(|_| (StatusCode::BAD_GATEWAY, "invalid upstream response"))
}
pub async fn serve(listener: UnixListener, guard: Arc<Guard>, session: Uuid) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(10))
        .build()?;
    let app = Router::new().fallback(forward).with_state(Gateway {
        guard,
        session,
        client,
    });
    axum::serve(listener, app).await?;
    Ok(())
}
pub fn validate(endpoints: &BTreeMap<String, Endpoint>) -> anyhow::Result<()> {
    for (name, endpoint) in endpoints {
        anyhow::ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
            "invalid endpoint name"
        );
        endpoint.validate()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::GuardConfig, protocol::Registration};
    use cerynth_security::{Capability, CapabilitySet, GuardSession, Mode, SessionMode};
    #[tokio::test]
    async fn fixed_endpoint_strips_client_auth_blocks_unknown_endpoints_and_redirects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let backend = tokio::spawn(async move {
            let app = Router::new().fallback(|req: Request| async move {
                if req.uri().path() == "/v1/models" {
                    return (
                        StatusCode::FOUND,
                        [("location", "http://evil.invalid/")],
                        "",
                    )
                        .into_response();
                }
                assert_eq!(
                    req.headers().get("authorization").unwrap(),
                    "Bearer vault-canary"
                );
                assert!(req.headers().get("x-api-key").is_none());
                "model-response".into_response()
            });
            axum::serve(listener, app).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "local".into(),
            Endpoint {
                base_url: origin,
                credential: Some("provider".into()),
            },
        );
        let vault = dir.path().join("vault");
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir(&vault).unwrap();
        std::fs::set_permissions(&vault, std::fs::Permissions::from_mode(0o700)).unwrap();
        let secret_file = vault.join("provider.json");
        std::fs::write(&secret_file, serde_json::json!({"kind":"http_header", "header":"authorization", "value":"Bearer vault-canary", "base_url":endpoints["local"].base_url}).to_string()).unwrap();
        std::fs::set_permissions(&secret_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let guard = GuardConfig {
            runtime_dir: dir.path().join("run"),
            state_dir: dir.path().join("state"),
            endpoints,
            vault_dir: Some(vault),
            ..Default::default()
        }
        .build()
        .unwrap();
        let id = Uuid::new_v4();
        guard
            .register(Registration {
                session: GuardSession {
                    session_id: id,
                    parent_session_id: None,
                    mode: SessionMode::Managed,
                    workspace: dir.path().to_owned(),
                    capabilities: CapabilitySet(vec![
                        Capability::new(CapabilityKind::InferenceUse, "local", Mode::Allow),
                        Capability::new(CapabilityKind::CredentialUse, "provider", Mode::Allow),
                    ]),
                    harness: None,
                },
                remotes: Default::default(),
            })
            .unwrap();
        let gateway = Gateway {
            guard: guard.clone(),
            session: id,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .unwrap(),
        };
        let req = Request::builder()
            .method("POST")
            .uri("/local/v1/chat/completions")
            .header("authorization", "Bearer agent-key")
            .header("x-api-key", "agent-key")
            .body(Body::from("{}"))
            .unwrap();
        let response = perform(gateway.clone(), req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            "model-response"
        );
        for path in ["/unknown/v1/models", "/local/v1/models"] {
            let result = perform(
                gateway.clone(),
                Request::builder().uri(path).body(Body::empty()).unwrap(),
            )
            .await;
            assert!(result.is_err());
        }
        guard.end_session(id);
        assert!(
            perform(
                gateway,
                Request::builder()
                    .uri("/local/v1/models")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .is_err()
        );
        backend.abort();
    }
    #[test]
    fn rejects_unsafe_endpoint_configuration() {
        for url in [
            "http://example.com/",
            "https://user:password@example.com/",
            "https://example.com/path",
            "file:///tmp/x",
            "https://example.com/?secret=x",
        ] {
            assert!(
                Endpoint {
                    base_url: url.into(),
                    credential: None
                }
                .validate()
                .is_err(),
                "{url}"
            );
        }
    }
}
