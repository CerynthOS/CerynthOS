//! Explicitly configured OpenAI-compatible chat endpoints; no automatic cloud fallback.
use crate::{InferenceMetrics, InferenceResult, Message};
use anyhow::{Context, Result, ensure};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::{net::IpAddr, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibleEndpoint {
    pub url: String,
    pub cloud_consent: bool,
    /// Name of a service environment variable. Never a literal credential.
    pub credential_env: Option<String>,
}

#[derive(Clone)]
pub struct CompatibleClient {
    client: Client,
    url: Url,
    credential: Option<String>,
}

impl CompatibleClient {
    /// # Errors
    /// Returns an error for nonconsensual remote routing or insecure URL settings.
    pub fn new(spec: &CompatibleEndpoint) -> Result<Self> {
        let mut url = Url::parse(&spec.url)?;
        let local = url
            .host_str()
            .unwrap_or_default()
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "endpoint credentials, query, and fragment are forbidden"
        );
        ensure!(
            (local && url.scheme() == "http") || (spec.cloud_consent && url.scheme() == "https"),
            "remote inference requires explicit consent and HTTPS"
        );
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        let credential = if let Some(name) = &spec.credential_env {
            ensure!(
                name.len() <= 128
                    && name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
                "invalid credential environment variable name"
            );
            Some(std::env::var(name).context("configured credential is unavailable")?)
        } else {
            None
        };
        Ok(Self {
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(120))
                .build()?,
            url,
            credential,
        })
    }

    /// # Errors
    /// Returns an error for unavailable, malformed, or oversized backend responses.
    pub async fn infer(
        &self,
        model: &str,
        prompt: &str,
        max_tokens: u32,
    ) -> Result<(InferenceResult, InferenceMetrics)> {
        let start = std::time::Instant::now();
        let mut request = self.client.post(self.url.join("chat/completions")?).json(
            &serde_json::json!({"model":model,"messages":[{"role":"user","content":prompt}],
                "max_tokens":max_tokens,"stream":false}),
        );
        if let Some(credential) = &self.credential {
            request = request.bearer_auth(credential);
        }
        let mut response = request
            .send()
            .await
            .context("compatible inference request failed")?;
        ensure!(
            response.status().is_success(),
            "backend returned HTTP {}",
            response.status()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 1024 * 1024,
                "compatible response exceeds limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        let response: serde_json::Value =
            serde_json::from_slice(&bytes).context("invalid compatible response")?;
        let content = response["choices"][0]["message"]["content"]
            .as_str()
            .context("missing assistant content")?;
        let count = response["usage"]["completion_tokens"].as_u64();
        let metrics = InferenceMetrics {
            queue_ms: 0.0,
            request_ms: start.elapsed().as_secs_f64() * 1000.0,
            ttft_ms: None,
            generated_tokens: count,
            tokens_per_second: None,
        };
        Ok((
            InferenceResult {
                model: model.into(),
                message: Message {
                    role: "assistant".into(),
                    content: content.into(),
                },
                done: true,
                prompt_eval_count: response["usage"]["prompt_tokens"].as_u64(),
                eval_count: count,
                total_duration: None,
                eval_duration: None,
                load_duration: None,
                prompt_eval_duration: None,
            },
            metrics,
        ))
    }

    /// # Errors
    /// Returns an error for unsupported routes or transport failure.
    pub async fn proxy(&self, route: &str, body: serde_json::Value) -> Result<reqwest::Response> {
        ensure!(
            matches!(route, "chat/completions" | "responses"),
            "unsupported compatible route"
        );
        let mut request = self.client.post(self.url.join(route)?).json(&body);
        if let Some(credential) = &self.credential {
            request = request.bearer_auth(credential);
        }
        request
            .send()
            .await
            .context("compatible proxy request failed")
    }
}
