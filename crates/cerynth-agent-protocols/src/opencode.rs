//! Supported `OpenCode` HTTP session endpoints. All server operations are explicit.
use anyhow::{Context, Result, ensure};
use reqwest::{Client, Method, Url};
use serde_json::{Value, json};
use std::{net::IpAddr, time::Duration};

#[derive(Clone)]
pub struct OpenCodeClient {
    client: Client,
    origin: Url,
    username: String,
    password: Option<String>,
}

impl OpenCodeClient {
    /// # Errors
    /// Returns an error for non-loopback origins, URL credentials, or missing explicit authentication.
    pub fn new(origin: &str, password_env: Option<&str>) -> Result<Self> {
        let password = password_env
            .map(std::env::var)
            .transpose()
            .context("configured OpenCode credential unavailable")?;
        Self::with_password(origin, password)
    }

    /// # Errors
    /// Returns an error for an invalid origin or an empty/oversized password.
    /// This constructor lets an owned bridge supply an ephemeral credential
    /// without mutating the process environment.
    pub fn with_password(origin: &str, password: Option<String>) -> Result<Self> {
        let origin = Url::parse(origin)?;
        ensure!(
            origin.scheme() == "http"
                && origin
                    .host_str()
                    .unwrap_or_default()
                    .trim_matches(['[', ']'])
                    .parse::<IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
                && origin.username().is_empty()
                && origin.password().is_none()
                && origin.query().is_none()
                && origin.fragment().is_none()
                && origin.path() == "/",
            "OpenCode must use an HTTP numeric loopback origin without URL credentials"
        );
        ensure!(
            password
                .as_ref()
                .is_none_or(|p| !p.is_empty() && p.len() <= 4096),
            "invalid OpenCode password"
        );
        Ok(Self {
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(120))
                .connect_timeout(Duration::from_secs(3))
                .build()?,
            origin,
            username: "opencode".into(),
            password,
        })
    }

    async fn request(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let mut request = self.client.request(method, self.origin.join(path)?);
        if let Some(password) = &self.password {
            request = request.basic_auth(&self.username, Some(password));
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request.send().await.context("OpenCode request failed")?;
        ensure!(
            response.status().is_success(),
            "OpenCode returned HTTP {}",
            response.status()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 4 * 1024 * 1024,
                "OpenCode response exceeds limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::from_slice(&bytes).context("invalid OpenCode JSON")
        }
    }

    /// # Errors
    /// Returns an error if the health endpoint is unavailable.
    pub async fn health(&self) -> Result<Value> {
        self.request(Method::GET, "global/health", None).await
    }

    /// # Errors
    /// Returns an error if local session discovery fails.
    pub async fn sessions(&self) -> Result<Value> {
        self.request(Method::GET, "session", None).await
    }

    /// # Errors
    /// Returns an error when session creation fails.
    pub async fn create_session(&self, title: &str) -> Result<Value> {
        ensure!(
            !title.is_empty() && title.len() <= 256,
            "invalid session title"
        );
        self.request(Method::POST, "session", Some(json!({"title":title})))
            .await
    }

    fn path(id: &str, suffix: &str) -> Result<String> {
        ensure!(
            !id.is_empty()
                && id.len() <= 256
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "invalid native session ID"
        );
        Ok(format!("session/{id}/{suffix}"))
    }

    /// # Errors
    /// Returns an error on invalid input, authorization failure, or agent failure.
    pub async fn prompt(&self, id: &str, provider: &str, model: &str, text: &str) -> Result<Value> {
        ensure!(!text.is_empty() && text.len() <= 32768, "invalid prompt");
        self.request(Method::POST,&Self::path(id,"message")?,Some(json!({
            "model":{"providerID":provider,"modelID":model},"parts":[{"type":"text","text":text}]}))).await
    }

    /// # Errors
    /// Returns an error if the server cannot abort the session.
    pub async fn cancel(&self, id: &str) -> Result<Value> {
        self.request(Method::POST, &Self::path(id, "abort")?, Some(json!({})))
            .await
    }

    /// # Errors
    /// Returns an error if message history is unavailable.
    pub async fn messages(&self, id: &str) -> Result<Value> {
        self.request(Method::GET, &Self::path(id, "message")?, None)
            .await
    }
}
