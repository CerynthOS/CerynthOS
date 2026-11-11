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
    #[serde(default)]
    pub cache_isolation: CacheIsolation,
}

/// Owner-declared backend support; generic compatible APIs prove no cache isolation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheIsolation {
    #[default]
    SharedTrustDomain,
    /// Validated with vLLM 0.25.0 chat requests; never inferred from HTTP compatibility.
    VllmPerGrantSalt,
}

#[derive(Clone)]
pub struct CompatibleClient {
    client: Client,
    url: Url,
    credential: Option<String>,
    cache_isolation: CacheIsolation,
    cache_salt: Option<String>,
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
        ensure!(
            spec.cache_isolation == CacheIsolation::SharedTrustDomain || local,
            "vLLM cache salting requires an explicitly configured local backend"
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
            cache_isolation: spec.cache_isolation,
            cache_salt: None,
        })
    }

    /// Bind an owner-authorized opaque grant scope, never a client-selected salt.
    #[must_use]
    pub fn for_cache_scope(&self, scope: String) -> Self {
        let mut scoped = self.clone();
        if scoped.cache_isolation == CacheIsolation::VllmPerGrantSalt {
            scoped.cache_salt = Some(scope);
        }
        scoped
    }

    fn scoped_body(&self, mut body: serde_json::Value) -> Result<serde_json::Value> {
        ensure!(
            body.get("cache_salt")
                .is_none_or(serde_json::Value::is_null),
            "cache salt must be assigned by the coordinator"
        );
        if self.cache_isolation == CacheIsolation::VllmPerGrantSalt {
            let salt = self
                .cache_salt
                .as_ref()
                .context("cache-isolated backend requires a grant scope")?;
            ensure!(
                salt.len() == 64 && salt.bytes().all(|c| c.is_ascii_hexdigit()),
                "invalid private cache scope"
            );
            body["cache_salt"] = serde_json::json!(salt);
        }
        Ok(body)
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
        let body = self.scoped_body(serde_json::json!({"model":model,
            "messages":[{"role":"user","content":prompt}], "max_tokens":max_tokens,"stream":false}))?;
        let mut request = self
            .client
            .post(self.url.join("chat/completions")?)
            .json(&body);
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
        ensure!(
            route == "chat/completions"
                || self.cache_isolation == CacheIsolation::SharedTrustDomain,
            "cache-isolated Responses transport has not been validated"
        );
        let body = self.scoped_body(body)?;
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
