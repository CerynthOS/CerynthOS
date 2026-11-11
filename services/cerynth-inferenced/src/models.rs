use crate::{Backend, Coordinator};
use anyhow::{Context, Result, ensure};
use cerynth_model_api::OllamaClient;
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

impl Coordinator {
    pub(crate) async fn registry(
        &self,
        endpoint_id: String,
        model: Option<String>,
    ) -> Result<Value> {
        let (spec, backend) = self
            .state
            .lock()
            .await
            .endpoints
            .get(&endpoint_id)
            .context("unknown endpoint")?
            .clone();
        let Backend::Ollama(client) = backend else {
            anyhow::bail!("backend model discovery is unsupported");
        };
        if let Some(name) = &model {
            ensure!(!name.is_empty() && name.len() <= 256, "invalid model name");
        }
        let installed = client.models().await?;
        let running = client.running_models().await?;
        let details = if let Some(name) = &model {
            Some(client.model_details(name).await?)
        } else {
            None
        };
        let now = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
            .unwrap_or(u64::MAX);
        let mut state = self.state.lock().await;
        ensure!(
            serde_json::to_value(
                &state
                    .endpoints
                    .get(&endpoint_id)
                    .context("endpoint removed")?
                    .0
            )? == serde_json::to_value(spec)?,
            "endpoint changed during observation; retry discovery"
        );
        let previous = state.models.clone();
        let mut next = previous.clone();
        next.refresh(&endpoint_id, &installed, &running, now)?;
        if let (Some(name), Some(details)) = (&model, &details) {
            next.enrich(&endpoint_id, name, details)?;
        }
        let records: Vec<_> = next
            .records
            .values()
            .filter(|r| r.endpoint_id == endpoint_id)
            .cloned()
            .collect();
        state.models = next;
        if let Err(error) = self.persist(&state) {
            state.models = previous;
            return Err(error);
        }
        Ok(json!({"endpoint_id":endpoint_id,"models":records,"source":"backend_observation"}))
    }

    pub(crate) async fn discover(candidates: Vec<String>) -> Result<Value> {
        ensure!(
            !candidates.is_empty() && candidates.len() <= 8,
            "discovery needs 1..8 explicit loopback origins"
        );
        // Validate every target before initiating any request. Never scan LANs,
        // import credentials, register a provider or download a model implicitly.
        let clients: Vec<_> = candidates
            .into_iter()
            .map(|url| Ok((url.clone(), OllamaClient::new(&url)?)))
            .collect::<Result<_>>()?;
        let mut tasks = tokio::task::JoinSet::new();
        for (url, client) in clients {
            tasks.spawn(async move {
                let result = tokio::time::timeout(Duration::from_secs(1), client.models()).await;
                let reachable = result.as_ref().is_ok_and(std::result::Result::is_ok);
                let count = result.ok().and_then(Result::ok).map(|models| models.len());
                json!({"url":url,"backend":"ollama","reachable":reachable,"model_count":count})
            });
        }
        let mut endpoints = Vec::new();
        while let Some(result) = tasks.join_next().await {
            endpoints.push(result?);
        }
        endpoints.sort_by(|a, b| a["url"].as_str().cmp(&b["url"].as_str()));
        Ok(json!({"candidates":endpoints,"registered":false}))
    }
}
