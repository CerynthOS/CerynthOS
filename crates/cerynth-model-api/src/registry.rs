//! Normalized, bounded model metadata. Unavailable observations stay unknown.
use crate::Model;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn key(endpoint: &str, name: &str) -> String {
    format!("{}:{endpoint}{name}", endpoint.len())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRecord {
    pub endpoint_id: String,
    pub backend: String,
    pub name: String,
    pub digest: String,
    pub weight_bytes: u64,
    pub quantization: Option<String>,
    pub family: Option<String>,
    pub parameter_size: Option<String>,
    pub architecture_context_limit: Option<u64>,
    pub configured_context_length: Option<u64>,
    pub loaded: Option<bool>,
    pub allocated_memory_bytes: Option<u64>,
    pub allocated_vram_bytes: Option<u64>,
    /// Generic Ollama metadata does not prove a physical GPU identity.
    pub device: Option<String>,
    pub capabilities: Vec<String>,
    /// License text is not silently converted into a fabricated SPDX identifier.
    pub license_available: Option<bool>,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRegistry {
    pub records: BTreeMap<String, ModelRecord>,
}

impl ModelRegistry {
    /// # Errors
    /// Rejects excessive inventories, invalid names or duplicate backend entries.
    pub fn refresh(
        &mut self,
        endpoint: &str,
        installed: &[Model],
        running: &Value,
        now: u64,
    ) -> Result<()> {
        ensure!(
            installed.len() <= 256,
            "model inventory exceeds 256 entries"
        );
        let mut next = self.records.clone();
        let mut names = BTreeSet::new();
        for model in installed {
            ensure!(
                !model.name.is_empty()
                    && model.name.len() <= 256
                    && !model.name.chars().any(char::is_control)
                    && names.insert(model.name.clone()),
                "invalid or duplicate model identity"
            );
            let key = key(endpoint, &model.name);
            let details = &model.details;
            let text = |key: &str| details[key].as_str().map(|s| s.chars().take(128).collect());
            let active = running["models"].as_array().and_then(|models| {
                models
                    .iter()
                    .find(|entry| entry["name"] == model.name || entry["model"] == model.name)
            });
            let previous = next
                .get(&key)
                .filter(|record| !model.digest.is_empty() && record.digest == model.digest);
            let record = ModelRecord {
                endpoint_id: endpoint.into(),
                backend: "ollama".into(),
                name: model.name.clone(),
                digest: model.digest.clone(),
                weight_bytes: model.size,
                quantization: text("quantization_level"),
                family: text("family"),
                parameter_size: text("parameter_size"),
                architecture_context_limit: previous.and_then(|r| r.architecture_context_limit),
                configured_context_length: active.and_then(|v| v["context_length"].as_u64()),
                loaded: Some(active.is_some()),
                allocated_memory_bytes: active.and_then(|v| v["size"].as_u64()),
                allocated_vram_bytes: active.and_then(|v| v["size_vram"].as_u64()),
                device: None,
                capabilities: previous.map_or_else(Vec::new, |r| r.capabilities.clone()),
                license_available: previous.and_then(|r| r.license_available),
                observed_at_ms: now,
            };
            next.insert(key, record);
        }
        next.retain(|_, record| record.endpoint_id != endpoint || names.contains(&record.name));
        ensure!(
            next.len() <= 256,
            "global model registry exceeds 256 entries"
        );
        self.records = next;
        Ok(())
    }

    /// # Errors
    /// Rejects details for a model missing from its current backend inventory.
    pub fn enrich(&mut self, endpoint: &str, name: &str, details: &Value) -> Result<()> {
        let record = self
            .records
            .get_mut(&key(endpoint, name))
            .ok_or_else(|| anyhow::anyhow!("model is not installed on this endpoint"))?;
        record.architecture_context_limit = details["model_info"].as_object().and_then(|info| {
            info.iter()
                .filter(|(key, _)| key.ends_with(".context_length"))
                .filter_map(|(_, value)| value.as_u64())
                .min()
        });
        record.capabilities = details["capabilities"]
            .as_array()
            .map_or_else(Vec::new, |values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .take(32)
                    .map(|s| s.chars().take(128).collect())
                    .collect()
            });
        record.license_available = details
            .get("license")
            .map(|value| value.as_str().is_some_and(|s| !s.is_empty()));
        Ok(())
    }

    /// Persisted metadata is useful after restart; residency must be observed again.
    pub fn invalidate_residency(&mut self) {
        for record in self.records.values_mut() {
            record.loaded = None;
            record.allocated_memory_bytes = None;
            record.allocated_vram_bytes = None;
            record.configured_context_length = None;
            record.device = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn weight_size_context_capacity_and_observed_residency_are_separate() -> Result<()> {
        let models: Vec<Model> =
            serde_json::from_value(json!([{"name":"model","size":100,"digest":"sha",
            "details":{"quantization_level":"Q4_K_M","family":"qwen3"}}]))?;
        let mut registry = ModelRegistry::default();
        registry.refresh(
            "local",
            &models,
            &json!({"models":[{"name":"model","size":300,"size_vram":200,"context_length":4096}]}),
            1,
        )?;
        registry.enrich("local","model",&json!({"model_info":{"qwen3.context_length":32768},"capabilities":["completion"],"license":"unstructured license"}))?;
        let record = &registry.records["5:localmodel"];
        assert_eq!(record.weight_bytes, 100);
        assert_eq!(record.allocated_vram_bytes, Some(200));
        assert_eq!(record.architecture_context_limit, Some(32768));
        assert_eq!(record.configured_context_length, Some(4096));
        assert!(record.device.is_none());
        registry.invalidate_residency();
        assert!(registry.records["5:localmodel"].loaded.is_none());
        assert_eq!(
            registry.records["5:localmodel"].architecture_context_limit,
            Some(32768)
        );
        Ok(())
    }
}
