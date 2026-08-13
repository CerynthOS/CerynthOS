//! Rules-based adaptive scheduling policy for `CerynthOS`.
//!
//! This crate intentionally operates in shadow mode: it evaluates telemetry
//! and produces a recommendation, but it never starts, stops, or modifies
//! the scheduler.

use cerynth_ipc::Profile;
use serde::{Deserialize, Serialize};

/// Telemetry presented to the adaptive policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyInput {
    pub cpu_usage_percent: f64,
    pub load_1m: f64,
    pub runnable_tasks: u64,
    pub context_switch_rate: f64,
    pub short_lived_process_rate: f64,
    pub current_profile: Profile,
}

/// Recommendation produced by the policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyDecision {
    pub recommended_profile: Profile,
    pub confidence: f64,
    pub reason: String,
}

/// Policy evaluation interface.
pub trait Policy {
    fn evaluate(&mut self, input: &PolicyInput) -> PolicyDecision;
}

/// Initial rules-based adaptive policy.
///
/// This is deliberately conservative. It recommends a profile only from
/// observable workload characteristics and never applies the recommendation.
#[derive(Debug, Clone)]
pub struct RulesBasedPolicy {
    min_confidence: f64,

    /// Number of consecutive evaluations required before accepting a
    /// different profile recommendation.
    min_dwell_samples: u32,

    /// Number of evaluations to wait after accepting a profile change before
    /// another profile change can be accepted.
    cooldown_samples: u32,

    /// Last profile accepted by the policy.
    accepted_profile: Option<Profile>,

    /// Candidate profile currently waiting for dwell confirmation.
    pending_profile: Option<Profile>,

    /// Number of consecutive evaluations supporting `pending_profile`.
    pending_count: u32,

    /// Remaining cooldown evaluations after a profile change.
    cooldown_remaining: u32,
}

impl Default for RulesBasedPolicy {
    fn default() -> Self {
        Self {
            min_confidence: 0.60,
            min_dwell_samples: 3,
            cooldown_samples: 5,
            accepted_profile: None,
            pending_profile: None,
            pending_count: 0,
            cooldown_remaining: 0,
        }
    }
}

impl RulesBasedPolicy {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_min_confidence(min_confidence: f64) -> Self {
        Self {
            min_confidence: min_confidence.clamp(0.0, 1.0),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_stability_controls(
        min_confidence: f64,
        min_dwell_samples: u32,
        cooldown_samples: u32,
    ) -> Self {
        Self {
            min_confidence: min_confidence.clamp(0.0, 1.0),
            min_dwell_samples: min_dwell_samples.max(1),
            cooldown_samples,
            ..Self::default()
        }
    }

    fn decision(
        &self,
        profile: Profile,
        confidence: f64,
        reason: impl Into<String>,
    ) -> PolicyDecision {
        PolicyDecision {
            recommended_profile: profile,
            confidence: confidence.max(self.min_confidence),
            reason: reason.into(),
        }
    }

    fn stabilize(&mut self, candidate: PolicyDecision, current_profile: Profile) -> PolicyDecision {
        // The scheduler's externally reported profile is the initial source
        // of truth when the policy is first evaluated.
        if self.accepted_profile.is_none() {
            self.accepted_profile = Some(current_profile);
        }

        let accepted = self.accepted_profile.unwrap_or(current_profile);

        // The recommendation already agrees with the accepted profile.
        if candidate.recommended_profile == accepted {
            self.pending_profile = None;
            self.pending_count = 0;

            if self.cooldown_remaining > 0 {
                self.cooldown_remaining -= 1;
            }

            return candidate;
        }

        // A cooldown prevents rapid profile oscillation.
        if self.cooldown_remaining > 0 {
            self.pending_profile = None;
            self.pending_count = 0;
            self.cooldown_remaining -= 1;

            return self.decision(
                accepted,
                candidate.confidence,
                format!(
                    "Holding {} during profile-change cooldown; candidate {}",
                    accepted, candidate.recommended_profile
                ),
            );
        }

        // Track consecutive evidence for the same candidate.
        if self.pending_profile == Some(candidate.recommended_profile) {
            self.pending_count = self.pending_count.saturating_add(1);
        } else {
            self.pending_profile = Some(candidate.recommended_profile);
            self.pending_count = 1;
        }

        // Require the recommendation to remain stable before accepting it.
        if self.pending_count < self.min_dwell_samples {
            return self.decision(
                accepted,
                candidate.confidence,
                format!(
                    "Holding {} until {} consecutive samples support {}; {}/{}",
                    accepted,
                    candidate.recommended_profile,
                    candidate.recommended_profile,
                    self.pending_count,
                    self.min_dwell_samples
                ),
            );
        }

        // Accept the new profile and begin the cooldown period.
        self.accepted_profile = Some(candidate.recommended_profile);
        self.pending_profile = None;
        self.pending_count = 0;
        self.cooldown_remaining = self.cooldown_samples;

        candidate
    }
}

impl Policy for RulesBasedPolicy {
    fn evaluate(&mut self, input: &PolicyInput) -> PolicyDecision {
        // Invalid telemetry must always fail safe to Balanced.
        if !input.cpu_usage_percent.is_finite()
            || !input.load_1m.is_finite()
            || !input.context_switch_rate.is_finite()
            || !input.short_lived_process_rate.is_finite()
        {
            return self.decision(
                Profile::Balanced,
                1.0,
                "Invalid telemetry; defaulting to balanced",
            );
        }

        // Generate the raw workload recommendation first.
        let candidate = if input.cpu_usage_percent >= 90.0
            && input.runnable_tasks >= 4
            && input.context_switch_rate < 200.0
        {
            self.decision(
                Profile::Performance,
                0.85,
                "Very high CPU utilisation with sustained runnable pressure and limited context switching",
            )
        } else if input.cpu_usage_percent >= 60.0
            && input.runnable_tasks >= 4
            && input.context_switch_rate >= 100.0
        {
            self.decision(
                Profile::Interactive,
                0.80,
                "High runnable load with frequent context switches",
            )
        } else if input.cpu_usage_percent < 30.0 && input.runnable_tasks <= 1 {
            self.decision(
                Profile::Balanced,
                0.90,
                "Low CPU utilisation and low runnable load",
            )
        } else {
            self.decision(
                Profile::Balanced,
                0.65,
                "Workload does not strongly match another profile",
            )
        };

        // Apply dwell-time and cooldown protection to the raw
        // recommendation.
        self.stabilize(candidate, input.current_profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        cpu: f64,
        load: f64,
        runnable: u64,
        context_switch_rate: f64,
        short_lived: f64,
    ) -> PolicyInput {
        PolicyInput {
            cpu_usage_percent: cpu,
            load_1m: load,
            runnable_tasks: runnable,
            context_switch_rate,
            short_lived_process_rate: short_lived,
            current_profile: Profile::Balanced,
        }
    }

    #[test]
    fn low_load_recommends_balanced() {
        let mut policy = RulesBasedPolicy::new();

        let decision = policy.evaluate(&input(15.0, 0.2, 1, 100.0, 0.0));

        assert_eq!(decision.recommended_profile, Profile::Balanced);
        assert!(decision.confidence >= 0.60);
    }

    #[test]
    fn cpu_saturation_recommends_performance() {
        let mut policy = RulesBasedPolicy::new();

        let workload = input(100.0, 0.7, 5, 150.0, 0.0);

        policy.evaluate(&workload);
        policy.evaluate(&workload);

        let decision = policy.evaluate(&workload);

        assert_eq!(decision.recommended_profile, Profile::Performance);
    }

    #[test]
    fn interactive_workload_recommends_interactive() {
        let mut policy = RulesBasedPolicy::new();

        let workload = input(75.0, 1.0, 4, 220.0, 0.0);

        policy.evaluate(&workload);
        policy.evaluate(&workload);

        let decision = policy.evaluate(&workload);

        assert_eq!(decision.recommended_profile, Profile::Interactive);
    }

    #[test]
    fn performance_dominates_when_context_switching_is_low() {
        let mut policy = RulesBasedPolicy::new();

        let workload = input(98.0, 1.0, 5, 150.0, 0.0);

        policy.evaluate(&workload);
        policy.evaluate(&workload);

        let decision = policy.evaluate(&workload);

        assert_eq!(decision.recommended_profile, Profile::Performance);
    }

    #[test]
    fn ambiguous_workload_defaults_to_balanced() {
        let mut policy = RulesBasedPolicy::new();

        let decision = policy.evaluate(&input(45.0, 1.5, 2, 200.0, 0.0));

        assert_eq!(decision.recommended_profile, Profile::Balanced);
    }

    #[test]
    fn invalid_telemetry_fails_safe() {
        let mut policy = RulesBasedPolicy::new();

        let decision = policy.evaluate(&input(f64::NAN, 1.0, 2, 100.0, 0.0));

        assert_eq!(decision.recommended_profile, Profile::Balanced);
        assert!(decision.reason.contains("Invalid telemetry"));
    }

    #[test]
    fn recommendation_requires_dwell_samples() {
        let mut policy = RulesBasedPolicy::with_stability_controls(0.60, 3, 0);

        let workload = input(100.0, 0.7, 5, 150.0, 0.0);

        let first = policy.evaluate(&workload);
        assert_eq!(first.recommended_profile, Profile::Balanced);
        assert!(first.reason.contains("1/3"));

        let second = policy.evaluate(&workload);
        assert_eq!(second.recommended_profile, Profile::Balanced);
        assert!(second.reason.contains("2/3"));

        let third = policy.evaluate(&workload);
        assert_eq!(third.recommended_profile, Profile::Performance);
    }

    #[test]
    fn cooldown_prevents_immediate_oscillation() {
        let mut policy = RulesBasedPolicy::with_stability_controls(0.60, 1, 3);

        let performance = input(100.0, 0.7, 5, 150.0, 0.0);

        let first = policy.evaluate(&performance);
        assert_eq!(first.recommended_profile, Profile::Performance);

        let balanced = input(10.0, 0.1, 1, 10.0, 0.0);

        let second = policy.evaluate(&balanced);
        assert_eq!(second.recommended_profile, Profile::Performance);
        assert!(second.reason.contains("cooldown"));

        let third = policy.evaluate(&balanced);
        assert_eq!(third.recommended_profile, Profile::Performance);

        let fourth = policy.evaluate(&balanced);
        assert_eq!(fourth.recommended_profile, Profile::Performance);

        let fifth = policy.evaluate(&balanced);
        assert_eq!(fifth.recommended_profile, Profile::Balanced);
    }

    #[test]
    fn changing_candidate_resets_dwell_counter() {
        let mut policy = RulesBasedPolicy::with_stability_controls(0.60, 3, 0);

        let performance = input(100.0, 0.7, 5, 150.0, 0.0);
        let interactive = input(75.0, 1.0, 4, 220.0, 0.0);

        let first = policy.evaluate(&performance);
        assert_eq!(first.recommended_profile, Profile::Balanced);

        let second = policy.evaluate(&interactive);
        assert_eq!(second.recommended_profile, Profile::Balanced);
        assert!(second.reason.contains("1/3"));

        let third = policy.evaluate(&interactive);
        assert_eq!(third.recommended_profile, Profile::Balanced);
        assert!(third.reason.contains("2/3"));
    }

    #[test]
    fn decision_is_serializable() {
        let decision = PolicyDecision {
            recommended_profile: Profile::Interactive,
            confidence: 0.84,
            reason: "High runnable load".to_string(),
        };

        let json = serde_json::to_string(&decision).unwrap();

        assert!(json.contains("\"recommended_profile\":\"interactive\""));
        assert!(json.contains("\"confidence\":0.84"));
    }
}
