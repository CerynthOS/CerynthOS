//! Bounded Ollama HTTP client. Loopback transport is not a sandbox guarantee.

use std::{net::IpAddr, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

pub mod admission;
pub mod cache;
pub mod compatible;
pub mod placement;
pub mod registry;

/// Canonical backend origin used to reject aliases that bypass tenancy policy.
///
/// # Errors
/// Returns an error for malformed endpoint URLs.
pub fn origin_key(endpoint: &str) -> Result<String> {
    Ok(Url::parse(endpoint)?.origin().ascii_serialization())
}

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct OllamaClient {
    client: Client,
    endpoint: Url,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    /// Downloaded weight size, not a VRAM reservation or enforced limit.
    pub size: u64,
    #[serde(default)]
    pub digest: String,
    #[serde(default)]
    pub details: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InferenceResult {
    pub model: String,
    pub message: Message,
    pub done: bool,
    pub prompt_eval_count: Option<u64>,
    pub eval_count: Option<u64>,
    pub total_duration: Option<u64>,
    pub eval_duration: Option<u64>,
    pub load_duration: Option<u64>,
    pub prompt_eval_duration: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceMetrics {
    pub queue_ms: f64,
    pub request_ms: f64,
    pub ttft_ms: Option<f64>,
    pub generated_tokens: Option<u64>,
    pub tokens_per_second: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl OllamaClient {
    /// Numeric loopback only, with no proxies, redirects, or URL credentials.
    ///
    /// # Errors
    /// Returns an error for unsupported endpoints or HTTP client construction.
    pub fn new(endpoint: &str) -> Result<Self> {
        let endpoint = Url::parse(endpoint).context("invalid endpoint URL")?;
        let host = endpoint
            .host_str()
            .unwrap_or_default()
            .trim_matches(['[', ']']);
        ensure!(
            host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()),
            "endpoint must use a numeric loopback address"
        );
        ensure!(
            endpoint.scheme() == "http"
                && endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.query().is_none()
                && endpoint.fragment().is_none()
                && endpoint.path() == "/",
            "endpoint must be an HTTP loopback origin without credentials or a path"
        );
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self { client, endpoint })
    }

    async fn decode<T: serde::de::DeserializeOwned>(mut response: reqwest::Response) -> Result<T> {
        // Do not include untrusted response bodies or prompts in error messages.
        ensure!(
            response.status().is_success(),
            "backend returned HTTP {}",
            response.status()
        );
        if response
            .content_length()
            .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
        {
            bail!("backend response exceeds limit");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("backend response read failed")?
        {
            ensure!(
                bytes.len() + chunk.len() <= MAX_RESPONSE_BYTES,
                "backend response exceeds limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).context("invalid backend response")
    }

    /// List models already installed on the configured endpoint; never pull one.
    ///
    /// # Errors
    /// Returns an error if the backend is unavailable or returns invalid data.
    pub async fn models(&self) -> Result<Vec<Model>> {
        #[derive(Deserialize)]
        struct Models {
            models: Vec<Model>,
        }
        let response = self
            .client
            .get(self.endpoint.join("api/tags")?)
            .send()
            .await
            .context("cannot reach Ollama endpoint")?;
        Ok(Self::decode::<Models>(response).await?.models)
    }

    /// Issue one bounded, non-streaming request with no tools.
    ///
    /// # Errors
    /// Returns an error on transport failure, timeout, or invalid/incomplete data.
    pub async fn infer(&self, model: &str, prompt: &str) -> Result<InferenceResult> {
        let response = self
            .client
            .post(self.endpoint.join("api/chat")?)
            .json(&serde_json::json!({
                "model": model,
                "messages": [{"role":"user", "content":prompt}],
                "stream":false,
                "options":{"num_predict":512}
            }))
            .send()
            .await
            .context("Ollama inference request failed")?;
        let result: InferenceResult = Self::decode(response).await?;
        ensure!(
            result.done && result.message.role == "assistant",
            "incomplete backend response"
        );
        Ok(result)
    }

    /// Measure time to first non-empty content chunk and engine-reported throughput.
    ///
    /// # Errors
    /// Returns an error for failed, oversized, malformed, or incomplete streams.
    pub async fn infer_streaming(
        &self,
        model: &str,
        prompt: &str,
        max_tokens: u32,
    ) -> Result<(InferenceResult, InferenceMetrics)> {
        ensure!(
            (1..=4096).contains(&max_tokens),
            "invalid output token limit"
        );
        let start = std::time::Instant::now();
        let mut response = self
            .client
            .post(self.endpoint.join("api/chat")?)
            .json(
                &serde_json::json!({"model":model,"messages":[{"role":"user","content":prompt}],
                "stream":true,"options":{"num_predict":max_tokens},"think":false}),
            )
            .send()
            .await
            .context("Ollama streaming request failed")?;
        ensure!(
            response.status().is_success(),
            "backend returned HTTP {}",
            response.status()
        );
        let mut pending = Vec::new();
        let mut total_bytes = 0;
        let mut content = String::new();
        let mut first = None;
        let mut final_result = None;
        while let Some(chunk) = response
            .chunk()
            .await
            .context("inference stream read failed")?
        {
            total_bytes += chunk.len();
            ensure!(
                total_bytes <= MAX_RESPONSE_BYTES,
                "inference stream exceeds limit"
            );
            pending.extend_from_slice(&chunk);
            while let Some(newline) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = pending.drain(..=newline).collect();
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let mut event: InferenceResult =
                    serde_json::from_slice(&line).context("invalid inference stream event")?;
                ensure!(
                    event.message.role == "assistant",
                    "unexpected stream message role"
                );
                if !event.message.content.is_empty() {
                    first.get_or_insert_with(|| start.elapsed().as_secs_f64() * 1000.0);
                    content.push_str(&event.message.content);
                }
                if event.done {
                    ensure!(final_result.is_none(), "duplicate inference completion");
                    event.message.content.clone_from(&content);
                    final_result = Some(event);
                }
            }
        }
        ensure!(
            pending.iter().all(u8::is_ascii_whitespace),
            "unterminated inference stream event"
        );
        let result = final_result.context("inference stream ended without completion")?;
        let throughput = result
            .eval_count
            .zip(result.eval_duration)
            .filter(|(_, duration)| *duration > 0)
            .and_then(|(count, duration)| {
                Some(
                    f64::from(u32::try_from(count).ok()?)
                        / Duration::from_nanos(duration).as_secs_f64(),
                )
            });
        let metrics = InferenceMetrics {
            queue_ms: 0.0,
            request_ms: start.elapsed().as_secs_f64() * 1000.0,
            ttft_ms: first,
            generated_tokens: result.eval_count,
            tokens_per_second: throughput,
        };
        Ok((result, metrics))
    }

    /// Query backend metadata without importing credentials or loading models.
    ///
    /// # Errors
    /// Returns an error on backend failure or oversized/invalid metadata.
    pub async fn model_details(&self, model: &str) -> Result<serde_json::Value> {
        let response = self
            .client
            .post(self.endpoint.join("api/show")?)
            .json(&serde_json::json!({"model":model}))
            .send()
            .await?;
        Self::decode(response).await
    }

    /// Query loaded models; reported `size_vram` is observational, not a limit.
    ///
    /// # Errors
    /// Returns an error if the backend is unavailable or returns invalid data.
    pub async fn running_models(&self) -> Result<serde_json::Value> {
        Self::decode(
            self.client
                .get(self.endpoint.join("api/ps")?)
                .send()
                .await?,
        )
        .await
    }

    /// Ask the backend to release one model; use only after checking active requests.
    ///
    /// # Errors
    /// Returns an error if the model cannot be unloaded or the backend fails.
    pub async fn unload(&self, model: &str) -> Result<serde_json::Value> {
        let response = self
            .client
            .post(self.endpoint.join("api/generate")?)
            .json(&serde_json::json!({"model":model,"keep_alive":0}))
            .send()
            .await?;
        Self::decode(response).await
    }

    /// Forward a validated chat/Responses request while keeping endpoint selection trusted.
    ///
    /// # Errors
    /// Returns an error for unsupported routes or transport failure.
    pub async fn proxy(&self, route: &str, body: serde_json::Value) -> Result<reqwest::Response> {
        ensure!(
            matches!(route, "chat/completions" | "responses"),
            "unsupported inference route"
        );
        self.client
            .post(self.endpoint.join(&format!("v1/{route}"))?)
            .json(&body)
            .send()
            .await
            .context("Ollama proxy request failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_remote_credential_and_ambiguous_urls() {
        for endpoint in [
            "https://example.com",
            "http://localhost:11434",
            "http://0.0.0.0:11434",
            "http://user:secret@127.0.0.1:11434",
            "http://127.0.0.1/api",
            "http://127.0.0.1/?token=x",
        ] {
            assert!(OllamaClient::new(endpoint).is_err(), "{endpoint}");
        }
        assert!(OllamaClient::new("http://127.0.0.1:11434").is_ok());
        assert!(OllamaClient::new("http://[::1]:11434").is_ok());
    }

    #[tokio::test]
    async fn rejects_redirects_http_errors_and_unbounded_bodies() -> Result<()> {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };

        let cases = [
            (
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1\r\nContent-Length: 0\r\n\r\n"
                    .to_string(),
                "HTTP 302",
            ),
            (
                "HTTP/1.1 503 Unavailable\r\nContent-Length: 18\r\n\r\nprivate error body"
                    .to_string(),
                "HTTP 503",
            ),
            (
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                    MAX_RESPONSE_BYTES + 1
                ),
                "exceeds limit",
            ),
            (
                format!(
                    "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{}",
                    " ".repeat(MAX_RESPONSE_BYTES + 1)
                ),
                "exceeds limit",
            ),
        ];
        for (response, expected) in cases {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let endpoint = format!("http://{}", listener.local_addr()?);
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 1024];
                assert!(stream.read(&mut buffer).await.unwrap() > 0);
                // A bounded client may close before the whole response is written.
                let _ = stream.write_all(response.as_bytes()).await;
            });
            let error = OllamaClient::new(&endpoint)?
                .models()
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "unexpected error: {error}");
            assert!(!error.contains("private error body"));
            server.await?;
        }
        Ok(())
    }
}
