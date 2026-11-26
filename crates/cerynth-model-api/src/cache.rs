//! `CacheWeaver` experiment: explicit recommendations over engine-owned cache metadata.
//! No cache content is shared, copied, or evicted by this policy module.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CachePhase {
    Interactive,
    InferenceWait,
    LongTool,
    Background,
    Idle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheEntry {
    pub id: String,
    pub trust_domain: String,
    pub model: String,
    pub bytes: u64,
    pub active_request: bool,
    pub priority: i32,
    pub phase: CachePhase,
    pub reload_ms: u64,
    pub recompute_ms: u64,
    pub estimated_reuse_after_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheCapabilities {
    pub evict: bool,
    pub offload_ram_bytes_per_ms: Option<u64>,
    pub offload_storage_bytes_per_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheAction {
    Retain,
    Evict,
    OffloadRam,
    OffloadStorage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheDecision {
    pub cache_id: String,
    pub action: CacheAction,
    pub estimated_cost_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachePlan {
    pub decisions: Vec<CacheDecision>,
    pub reclaimable_bytes: u64,
    pub remaining_shortfall_bytes: u64,
    pub experimental: bool,
}

/// Protect active/interactive caches; choose a supported action with the lowest
/// observed transfer/recompute cost for idle or long-tool sessions in one domain.
#[must_use]
pub fn plan(
    entries: &[CacheEntry],
    capabilities: &CacheCapabilities,
    trust_domain: &str,
    needed_bytes: u64,
) -> CachePlan {
    let mut candidates: Vec<_> = entries
        .iter()
        .filter(|entry| {
            entry.trust_domain == trust_domain
                && !entry.active_request
                && !matches!(
                    entry.phase,
                    CachePhase::Interactive | CachePhase::InferenceWait
                )
        })
        .filter_map(|entry| {
            let mut options = Vec::new();
            let recompute = entry.reload_ms.saturating_add(entry.recompute_ms);
            if capabilities.evict {
                options.push((recompute, CacheAction::Evict));
            }
            if let Some(speed) = capabilities
                .offload_ram_bytes_per_ms
                .filter(|speed| *speed > 0)
            {
                let cost = entry.bytes.div_ceil(speed).saturating_mul(2);
                if cost < recompute {
                    options.push((cost, CacheAction::OffloadRam));
                }
            }
            if let Some(speed) = capabilities
                .offload_storage_bytes_per_ms
                .filter(|speed| *speed > 0)
            {
                let cost = entry.bytes.div_ceil(speed).saturating_mul(2);
                if cost < recompute {
                    options.push((cost, CacheAction::OffloadStorage));
                }
            }
            let (cost, action) = options.into_iter().min_by_key(|(cost, _)| *cost)?;
            Some((entry, cost, action))
        })
        .collect();
    // Deterministic heuristic, not a learned prediction. Supplied reuse estimates
    // are evidence from the caller, never inferred from CPU idleness alone.
    candidates.sort_by_key(|(entry, cost, _)| {
        (
            entry.priority,
            std::cmp::Reverse(entry.estimated_reuse_after_ms.unwrap_or(u64::MAX)),
            *cost,
            entry.id.clone(),
        )
    });
    let mut decisions = Vec::new();
    let mut reclaimed = 0_u64;
    for (entry, cost, action) in candidates {
        if reclaimed >= needed_bytes {
            break;
        }
        decisions.push(CacheDecision {
            cache_id: entry.id.clone(),
            action,
            estimated_cost_ms: cost,
        });
        reclaimed = reclaimed.saturating_add(entry.bytes);
    }
    CachePlan {
        decisions,
        reclaimable_bytes: reclaimed,
        remaining_shortfall_bytes: needed_bytes.saturating_sub(reclaimed),
        experimental: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_and_other_tenant_caches_are_protected_and_offloading_requires_capability() {
        let base = CacheEntry {
            id: "idle".into(),
            trust_domain: "a".into(),
            model: "m".into(),
            bytes: 1000,
            active_request: false,
            priority: 0,
            phase: CachePhase::LongTool,
            reload_ms: 100,
            recompute_ms: 500,
            estimated_reuse_after_ms: Some(10000),
        };
        let mut active = base.clone();
        active.id = "active".into();
        active.active_request = true;
        let mut private = base.clone();
        private.id = "private".into();
        private.trust_domain = "b".into();
        let entries = [active, private, base];
        let unsupported = plan(&entries, &CacheCapabilities::default(), "a", 1000);
        assert_eq!(unsupported.remaining_shortfall_bytes, 1000);
        let supported = plan(
            &entries,
            &CacheCapabilities {
                evict: true,
                offload_ram_bytes_per_ms: Some(100),
                offload_storage_bytes_per_ms: None,
            },
            "a",
            2000,
        );
        assert_eq!(supported.decisions.len(), 1);
        assert_eq!(supported.decisions[0].cache_id, "idle");
        assert_eq!(supported.decisions[0].action, CacheAction::OffloadRam);
        assert_eq!(supported.remaining_shortfall_bytes, 1000);
    }
}
