//! Strict supported mode uses an owned process generation for each session realm.
//! Generic/external KV engines are refused, not labelled isolated without proof.
use crate::{Coordinator, EndpointSpec};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Policy {
    TrustDomain,
    ManagedProcess,
}

impl Policy {
    pub(crate) fn from_environment() -> Result<Self> {
        match std::env::var("CERYNTH_CACHE_PRIVACY").as_deref() {
            Err(_) | Ok("trust_domain") => Ok(Self::TrustDomain),
            Ok("managed_process") => Ok(Self::ManagedProcess),
            _ => anyhow::bail!("CERYNTH_CACHE_PRIVACY must be trust_domain or managed_process"),
        }
    }
    pub(crate) fn domain(self, grant: &crate::SessionGrant) -> String {
        if self == Self::ManagedProcess {
            format!(
                "session:{}:{:x}",
                grant.session_id,
                Sha256::digest(grant.trust_domain.as_bytes())
            )
        } else {
            grant.trust_domain.clone()
        }
    }
}

impl Coordinator {
    pub(crate) async fn require_private_backend(&self, endpoint: &EndpointSpec) -> Result<()> {
        if self.privacy != Policy::ManagedProcess {
            return Ok(());
        }
        let EndpointSpec::Ollama { id, url } = endpoint else {
            anyhow::bail!("strict cache privacy refuses unqualified external KV/cache engines");
        };
        ensure!(
            self.backends.owns(id, url).await,
            "strict cache privacy requires an exact coordinator-owned Ollama process generation"
        );
        Ok(())
    }

    pub(crate) async fn stop_private_backend(&self, id: &str) -> Result<Value> {
        let _exclusive = self
            .lifecycle
            .try_write()
            .context("inference is active or queued; stop sessions first")?;
        self.ensure_endpoint_not_leased(id).await?;
        if self.privacy == Policy::ManagedProcess {
            let mut state = self.state.lock().await;
            ensure!(
                !state.grants.values().any(|g| g.spec.endpoint_id == id),
                "revoke session grants before resetting a private cache process"
            );
            let (spec, _) = state.endpoints.get(id).context("unknown backend")?;
            self.require_private_backend(spec).await?;
            let EndpointSpec::Ollama { url, .. } = spec else {
                unreachable!()
            };
            let origin = cerynth_model_api::origin_key(url)?;
            self.backends.stop(id).await?; // Verified entire scope empty first.
            let previous = state.trust_domains.remove(&origin);
            if let Err(error) = self.persist(&state) {
                if let Some(domain) = previous {
                    state.trust_domains.insert(origin, domain);
                }
                return Err(error);
            }
        } else {
            self.backends.stop(id).await?;
        }
        Ok(
            json!({"stopped":id,"private_cache_generation_reset":self.privacy==Policy::ManagedProcess}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn strict_mode_rejects_unowned_and_compatible_backends_before_credentials_or_network()
    -> Result<()> {
        let mut coordinator = Coordinator::new(1);
        coordinator.privacy = Policy::ManagedProcess;
        let external = EndpointSpec::Ollama {
            id: "unowned".into(),
            url: "http://127.0.0.1:9".into(),
        };
        assert!(
            coordinator
                .admin(crate::AdminOperation::Configure { endpoint: external })
                .await
                .is_err()
        );
        let config = cerynth_model_api::compatible::CompatibleEndpoint {
            url: "http://127.0.0.1:9/v1".into(),
            cloud_consent: false,
            credential_env: Some("UNAVAILABLE_FIXTURE_CREDENTIAL".into()),
            cache_isolation: cerynth_model_api::compatible::CacheIsolation::default(),
        };
        let error = coordinator
            .admin(crate::AdminOperation::Configure {
                endpoint: EndpointSpec::Compatible {
                    id: "external-kv".into(),
                    config,
                },
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unqualified external KV"));
        Ok(())
    }
}
