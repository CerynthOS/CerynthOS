//! Advisory placement over owner-supplied telemetry and measured load costs.
//! Never changes model residency, routes requests, or grants device access.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementRequest {
    pub model_digest: String,
    pub context_length: u64,
    /// Calibrated resident allocation, not downloaded weight-file size.
    pub resident_bytes: u64,
    pub trust_domain: String,
    pub authorized_targets: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResidentModel {
    pub digest: String,
    pub context_length: u64,
    pub allocated_bytes: u64,
    pub active_requests: u32,
    pub protected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementTarget {
    pub id: String,
    pub trust_domain: String,
    /// Identifies the calibrated hardware/backend/version configuration.
    pub calibration_id: String,
    pub total_bytes: u64,
    pub free_bytes: Option<u64>,
    pub reserved_bytes: u64,
    pub desktop_margin_bytes: u64,
    pub observed_at_ms: u64,
    pub switch_supported: bool,
    pub resident: Option<ResidentModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchObservation {
    pub target_id: String,
    pub calibration_id: String,
    pub model_digest: String,
    pub context_length: u64,
    pub load_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchEstimate {
    pub samples: usize,
    pub median_ms: f64,
    pub minimum_ms: f64,
    pub maximum_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementAssessment {
    pub target_id: String,
    pub action: String,
    pub reason: String,
    pub switch_estimate: Option<SwitchEstimate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementPlan {
    pub selected: Option<String>,
    pub assessments: Vec<PlacementAssessment>,
    pub advisory: bool,
    pub changed_system_state: bool,
}

fn text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn assess(
    request: &PlacementRequest,
    target: &PlacementTarget,
    samples: &[SwitchObservation],
    now_ms: u64,
) -> PlacementAssessment {
    let rejection = |reason: &str| PlacementAssessment {
        target_id: target.id.clone(),
        action: "queue".into(),
        reason: reason.into(),
        switch_estimate: None,
    };
    if !request.authorized_targets.contains(&target.id)
        || target.trust_domain != request.trust_domain
    {
        return rejection("not_authorized");
    }
    if target.observed_at_ms > now_ms || now_ms - target.observed_at_ms > 60_000 {
        return rejection("stale_telemetry");
    }
    let Some(free) = target.free_bytes else {
        return rejection("memory_unknown");
    };
    if target.resident.as_ref().is_some_and(|resident| {
        resident.digest == request.model_digest
            && resident.context_length >= request.context_length
            && resident.allocated_bytes >= request.resident_bytes
    }) {
        return PlacementAssessment {
            target_id: target.id.clone(),
            action: "reuse".into(),
            reason: "matching_resident_model".into(),
            switch_estimate: Some(SwitchEstimate {
                samples: 0,
                median_ms: 0.0,
                minimum_ms: 0.0,
                maximum_ms: 0.0,
            }),
        };
    }
    let reclaim = if let Some(resident) = &target.resident {
        if resident.active_requests > 0 || resident.protected {
            return rejection("resident_model_protected");
        }
        if !target.switch_supported {
            return rejection("model_switch_unsupported");
        }
        resident.allocated_bytes
    } else {
        0
    };
    let available = free
        .saturating_add(reclaim)
        .min(target.total_bytes)
        .saturating_sub(target.reserved_bytes)
        .saturating_sub(target.desktop_margin_bytes);
    if request.resident_bytes > available {
        return rejection("insufficient_memory");
    }
    let mut durations: Vec<_> = samples
        .iter()
        .filter(|sample| {
            sample.target_id == target.id
                && sample.calibration_id == target.calibration_id
                && sample.model_digest == request.model_digest
                && sample.context_length == request.context_length
        })
        .map(|sample| sample.load_ms)
        .collect();
    if durations.is_empty() {
        return rejection("switch_cost_unknown");
    }
    durations.sort_by(f64::total_cmp);
    let middle = durations.len() / 2;
    let median = if durations.len() % 2 == 0 {
        durations[middle - 1].midpoint(durations[middle])
    } else {
        durations[middle]
    };
    PlacementAssessment {
        target_id: target.id.clone(),
        action: if target.resident.is_some() {
            "switch"
        } else {
            "load"
        }
        .into(),
        reason: "calibrated_capacity_available".into(),
        switch_estimate: Some(SwitchEstimate {
            samples: durations.len(),
            median_ms: median,
            minimum_ms: durations[0],
            maximum_ms: durations[durations.len() - 1],
        }),
    }
}

/// Compare candidates without inventing capacity, latency, or cross-tenant authority.
/// Fresh same-model residency needs no reload. Other choices need matching
/// calibration samples; active/protected models are never evicted by this plan.
/// # Errors
/// Rejects invalid, ambiguous, excessive, or nonfinite input observations.
pub fn plan(
    request: &PlacementRequest,
    targets: &[PlacementTarget],
    samples: &[SwitchObservation],
    now_ms: u64,
) -> Result<PlacementPlan> {
    ensure!(
        text(&request.model_digest)
            && text(&request.trust_domain)
            && request.context_length > 0
            && request.resident_bytes > 0
            && !request.authorized_targets.is_empty()
            && request.authorized_targets.len() <= 8
            && request.authorized_targets.iter().all(|id| text(id)),
        "invalid placement request"
    );
    ensure!(
        !targets.is_empty() && targets.len() <= 8 && samples.len() <= 64,
        "placement input exceeds limits"
    );
    let mut ids = BTreeSet::new();
    ensure!(
        targets.iter().all(|target| text(&target.id)
            && text(&target.trust_domain)
            && text(&target.calibration_id)
            && ids.insert(target.id.clone())
            && target.total_bytes > 0
            && target
                .free_bytes
                .is_none_or(|free| free <= target.total_bytes)
            && target.reserved_bytes <= target.total_bytes
            && target.desktop_margin_bytes <= target.total_bytes
            && target.resident.as_ref().is_none_or(|r| text(&r.digest)
                && r.context_length > 0
                && r.allocated_bytes <= target.total_bytes)),
        "invalid placement target"
    );
    ensure!(
        samples.iter().all(|s| text(&s.target_id)
            && text(&s.calibration_id)
            && text(&s.model_digest)
            && s.context_length > 0
            && s.load_ms.is_finite()
            && (0.0..=86_400_000.0).contains(&s.load_ms)),
        "invalid switching observation"
    );
    let assessments: Vec<_> = targets
        .iter()
        .map(|target| assess(request, target, samples, now_ms))
        .collect();
    let selected = assessments
        .iter()
        .filter(|candidate| candidate.action != "queue")
        .min_by(|a, b| {
            let a_cost = a
                .switch_estimate
                .as_ref()
                .map_or(f64::INFINITY, |s| s.median_ms);
            let b_cost = b
                .switch_estimate
                .as_ref()
                .map_or(f64::INFINITY, |s| s.median_ms);
            a_cost
                .total_cmp(&b_cost)
                .then_with(|| a.target_id.cmp(&b.target_id))
        })
        .map(|candidate| candidate.target_id.clone());
    Ok(PlacementPlan {
        selected,
        assessments,
        advisory: true,
        changed_system_state: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inputs() -> (PlacementRequest, PlacementTarget, Vec<SwitchObservation>) {
        (
            PlacementRequest {
                model_digest: "m".into(),
                context_length: 1024,
                resident_bytes: 40,
                trust_domain: "owner-a".into(),
                authorized_targets: BTreeSet::from(["gpu".into()]),
            },
            PlacementTarget {
                id: "gpu".into(),
                trust_domain: "owner-a".into(),
                calibration_id: "hardware-runtime-v1".into(),
                total_bytes: 100,
                free_bytes: Some(60),
                reserved_bytes: 10,
                desktop_margin_bytes: 10,
                observed_at_ms: 10,
                switch_supported: true,
                resident: None,
            },
            [20.0, 1000.0, 30.0]
                .map(|load_ms| SwitchObservation {
                    target_id: "gpu".into(),
                    calibration_id: "hardware-runtime-v1".into(),
                    model_digest: "m".into(),
                    context_length: 1024,
                    load_ms,
                })
                .into(),
        )
    }
    #[test]
    fn calibrated_medians_preserve_outliers_and_never_expand_authority() -> Result<()> {
        let (request, target, samples) = inputs();
        let result = plan(&request, std::slice::from_ref(&target), &samples, 10)?;
        assert_eq!(result.selected.as_deref(), Some("gpu"));
        let cost = result.assessments[0].switch_estimate.as_ref().unwrap();
        assert!(
            (cost.median_ms - 30.0).abs() < 1e-9,
            "median_ms = {}",
            cost.median_ms
        );
        assert!(
            (cost.maximum_ms - 1000.0).abs() < 1e-9,
            "maximum_ms = {}",
            cost.maximum_ms
        );
        let mut denied = target;
        denied.trust_domain = "owner-b".into();
        assert!(plan(&request, &[denied], &samples, 10)?.selected.is_none());
        Ok(())
    }
    #[test]
    fn missing_or_stale_evidence_and_active_models_do_not_become_free_capacity() -> Result<()> {
        let (request, mut target, samples) = inputs();
        target.free_bytes = None;
        assert!(
            plan(&request, &[target.clone()], &samples, 10)?
                .selected
                .is_none()
        );
        target.free_bytes = Some(60);
        assert!(
            plan(&request, &[target.clone()], &samples, 60_011)?
                .selected
                .is_none()
        );
        target.free_bytes = Some(20);
        target.resident = Some(ResidentModel {
            digest: "other".into(),
            context_length: 1024,
            allocated_bytes: 60,
            active_requests: 1,
            protected: false,
        });
        assert!(
            plan(&request, &[target.clone()], &samples, 10)?
                .selected
                .is_none()
        );
        target.resident.as_mut().unwrap().active_requests = 0;
        assert!(
            plan(&request, &[target.clone()], &[], 10)?
                .selected
                .is_none()
        );
        assert_eq!(
            plan(&request, &[target], &samples, 10)?.assessments[0].action,
            "switch"
        );
        Ok(())
    }
}
