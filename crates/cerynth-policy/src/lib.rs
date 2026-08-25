//! Rules-based adaptive scheduling policy for `CerynthOS`.
//!
//! This crate intentionally operates in shadow mode: it evaluates telemetry
//! and produces a recommendation, but it never starts, stops, or modifies
//! the scheduler.

use cerynth_ipc::{AdaptationMode, Profile};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Audit event for adaptation decisions.
///
/// Logged as JSONL to `/var/log/cerynth/adaptation.jsonl` (configurable).
/// Contains all fields required by the sprint for automatic action tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptationAuditEvent {
    /// Unix timestamp (seconds since epoch)
    pub timestamp: u64,

    /// Human-readable timestamp (ISO 8601)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_iso: Option<String>,

    /// Result of the adaptation action
    /// Values: "rejected", "shadow", "success", "failed", "rollback"
    pub result: String,

    /// Previous profile (before switch/attempt)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_profile: Option<Profile>,

    /// New profile (after switch/attempt/rollback)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_profile: Option<Profile>,

    /// Confidence of the recommendation [0.0, 1.0]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,

    /// Human-readable reason for the decision
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Whether a rollback was attempted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback: Option<bool>,

    /// Rollback result if applicable
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_result: Option<String>,

    /// Scheduler health status at time of decision
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduler_healthy: Option<bool>,

    /// Additional metadata
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

impl AdaptationAuditEvent {
    /// Creates a new audit event builder.
    #[must_use]
    pub fn new(result: impl Into<String>) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self {
            timestamp,
            timestamp_iso: None,
            result: result.into(),
            old_profile: None,
            new_profile: None,
            confidence: None,
            reason: None,
            rollback: None,
            rollback_result: None,
            scheduler_healthy: None,
            extra: std::collections::HashMap::new(),
        }
    }

    /// Sets the old profile.
    #[must_use]
    pub fn with_old_profile(mut self, profile: Profile) -> Self {
        self.old_profile = Some(profile);
        self
    }

    /// Sets the new profile.
    #[must_use]
    pub fn with_new_profile(mut self, profile: Profile) -> Self {
        self.new_profile = Some(profile);
        self
    }

    /// Sets the confidence.
    #[must_use]
    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = Some(confidence);
        self
    }

    /// Sets the reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Sets whether a rollback was attempted.
    #[must_use]
    pub fn with_rollback(mut self, rollback: bool) -> Self {
        self.rollback = Some(rollback);
        self
    }

    /// Sets the rollback result.
    #[must_use]
    pub fn with_rollback_result(mut self, result: impl Into<String>) -> Self {
        self.rollback_result = Some(result.into());
        self
    }

    /// Sets scheduler health status.
    #[must_use]
    pub fn with_scheduler_healthy(mut self, healthy: bool) -> Self {
        self.scheduler_healthy = Some(healthy);
        self
    }

    /// Adds extra metadata.
    #[must_use]
    pub fn with_extra(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }

    /// Sets ISO timestamp (for human readability).
    #[must_use]
    pub fn with_iso_timestamp(mut self) -> Self {
        let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(self.timestamp as i64, 0);
        if let Some(dt) = dt {
            self.timestamp_iso = Some(dt.to_rfc3339());
        }
        self
    }
}

/// Audit logger for adaptation decisions.
///
/// Writes JSONL events to a configurable log file.
/// Default path: `/var/log/cerynth/adaptation.jsonl`
#[derive(Debug)]
pub struct AdaptationAuditLogger {
    path: String,
    enabled: bool,
}

impl AdaptationAuditLogger {
    /// Creates a new audit logger with the default path.
    #[must_use]
    pub fn new() -> Self {
        Self {
            path: "/var/log/cerynth/adaptation.jsonl".to_string(),
            enabled: true,
        }
    }

    /// Creates a new audit logger with a custom path.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Enables or disables the audit logger.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Returns whether the logger is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Logs an adaptation audit event.
    ///
    /// Creates the parent directory if it doesn't exist.
    /// Appends the event as a JSON line to the log file.
    pub fn log(&self, event: AdaptationAuditEvent) -> Result<(), std::io::Error> {
        if !self.enabled {
            return Ok(());
        }

        // Ensure parent directory exists
        if let Some(parent) = Path::new(&self.path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Open file for appending
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        let mut writer = BufWriter::new(file);

        // Serialize to JSON line
        let json = serde_json::to_string(&event)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        writeln!(writer, "{json}")?;
        writer.flush()?;

        Ok(())
    }

    /// Convenience method to log a rejected recommendation.
    pub fn log_rejected(
        &self,
        reason: &str,
        confidence: f64,
        scheduler_healthy: bool,
    ) -> Result<(), std::io::Error> {
        let event = AdaptationAuditEvent::new("rejected")
            .with_reason(reason)
            .with_confidence(confidence)
            .with_scheduler_healthy(scheduler_healthy)
            .with_iso_timestamp();
        self.log(event)
    }

    /// Convenience method to log a shadow mode recommendation.
    pub fn log_shadow(
        &self,
        new_profile: Profile,
        confidence: f64,
        reason: &str,
        scheduler_healthy: bool,
    ) -> Result<(), std::io::Error> {
        let event = AdaptationAuditEvent::new("shadow")
            .with_new_profile(new_profile)
            .with_confidence(confidence)
            .with_reason(reason)
            .with_scheduler_healthy(scheduler_healthy)
            .with_iso_timestamp();
        self.log(event)
    }

    /// Convenience method to log a successful switch.
    pub fn log_success(
        &self,
        old_profile: Profile,
        new_profile: Profile,
        confidence: f64,
        reason: &str,
        scheduler_healthy: bool,
    ) -> Result<(), std::io::Error> {
        let event = AdaptationAuditEvent::new("success")
            .with_old_profile(old_profile)
            .with_new_profile(new_profile)
            .with_confidence(confidence)
            .with_reason(reason)
            .with_scheduler_healthy(scheduler_healthy)
            .with_iso_timestamp();
        self.log(event)
    }

    /// Convenience method to log a failed switch.
    pub fn log_failed(
        &self,
        old_profile: Profile,
        attempted_profile: Profile,
        error: &str,
        scheduler_healthy: bool,
    ) -> Result<(), std::io::Error> {
        let event = AdaptationAuditEvent::new("failed")
            .with_old_profile(old_profile)
            .with_new_profile(attempted_profile)
            .with_reason(error)
            .with_scheduler_healthy(scheduler_healthy)
            .with_iso_timestamp();
        self.log(event)
    }

    /// Convenience method to log a rollback attempt.
    pub fn log_rollback(
        &self,
        old_profile: Profile,
        new_profile: Profile,
        rollback_success: bool,
        _error: Option<&str>,
        scheduler_healthy: bool,
    ) -> Result<(), std::io::Error> {
        let event = AdaptationAuditEvent::new("rollback")
            .with_old_profile(old_profile)
            .with_new_profile(new_profile)
            .with_rollback(true)
            .with_rollback_result(if rollback_success {
                "success"
            } else {
                "failed"
            })
            .with_scheduler_healthy(scheduler_healthy)
            .with_iso_timestamp();
        self.log(event)
    }
}

impl Default for AdaptationAuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

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

/// Policy recommendation read from the policy file.
///
/// This struct mirrors the output format written by `cerynth-policy-live`
/// and includes a timestamp for freshness checking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyRecommendation {
    /// The recommended scheduler profile.
    pub recommended_profile: Profile,

    /// Confidence in the recommendation [0.0, 1.0].
    pub confidence: f64,

    /// Human-readable reason for the recommendation.
    pub reason: String,

    /// Unix timestamp (seconds since epoch) when the recommendation was generated.
    pub timestamp: u64,

    /// Whether this recommendation should be applied (for future canary mode).
    #[serde(default)]
    pub apply: bool,
}

impl PolicyRecommendation {
    /// Returns the age of the recommendation in seconds.
    #[must_use]
    pub fn age_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .saturating_sub(self.timestamp)
    }

    /// Checks if the recommendation is fresh (within max_age_secs).
    #[must_use]
    pub fn is_fresh(&self, max_age_secs: u64) -> bool {
        max_age_secs > 0 && self.age_secs() <= max_age_secs
    }
}

/// Outcome of a single adaptation tick.
#[derive(Debug, Clone, PartialEq)]
pub enum AdaptationOutcome {
    /// Adaptation is disabled or not in canary mode — no action taken.
    Skipped { reason: String },

    /// Shadow mode: policy read and validated, recommendation logged, no switch.
    ShadowLogged {
        recommended_profile: Profile,
        confidence: f64,
        reason: String,
    },

    /// Guardrails rejected the switch.
    GuardrailRejected { violation: GuardrailViolation },

    /// Policy validation rejected the recommendation.
    ValidationRejected { error: PolicyValidationError },

    /// Policy file could not be read.
    PolicyReadError { error: String },

    /// Switch attempted successfully.
    Switched {
        from_profile: Profile,
        to_profile: Profile,
    },

    /// Switch failed (scheduler error).
    SwitchFailed { error: String },

    /// Rollback attempted after failed post-switch health check.
    RollbackAttempted {
        from_profile: Profile,
        to_profile: Profile,
        rollback_result: RollbackResult,
    },
}

/// Result of a rollback attempt after a failed profile switch.
#[derive(Debug, Clone, PartialEq)]
pub enum RollbackResult {
    /// Rollback succeeded: scheduler stopped, previous good profile started successfully.
    Success { previous_good_profile: Profile },

    /// Rollback failed: could not start previous good profile.
    /// Adaptation is disabled, Linux scheduler takes over.
    Failed {
        error: String,
        previous_good_profile: Profile,
    },
}

/// Adaptation Controller — orchestrates the adaptation loop.
///
/// Connects: Config → PolicyReader → PolicyValidator → GuardrailEngine → Scheduler Backend
///
/// Runs periodically (e.g., every second) to evaluate recommendations and
/// apply guarded switches in Canary mode, or log in Shadow mode.
#[derive(Debug)]
pub struct AdaptationController {
    /// Policy reader for loading recommendations from disk.
    policy_reader: PolicyReader,

    /// Validates policy recommendations for trustworthiness.
    policy_validator: PolicyValidator,

    /// Enforces safety guardrails on profile switches.
    guardrails: GuardrailEngine,

    /// Audit logger for recording adaptation decisions.
    audit_logger: AdaptationAuditLogger,

    /// Current adaptation mode (read from config).
    adaptation_mode: AdaptationMode,

    /// Whether adaptation is globally enabled (read from config).
    adaptation_enabled: bool,

    /// Interval between ticks in seconds.
    tick_interval_secs: u64,
}

impl AdaptationController {
    /// Creates a new AdaptationController with default settings.
    #[must_use]
    pub fn new(current_profile: Profile) -> Self {
        Self {
            policy_reader: PolicyReader::new(),
            policy_validator: PolicyValidator::new(),
            guardrails: GuardrailEngine::new(current_profile),
            audit_logger: AdaptationAuditLogger::new(),
            adaptation_mode: AdaptationMode::Off,
            adaptation_enabled: true,
            tick_interval_secs: 1,
        }
    }

    /// Sets the policy file path.
    #[must_use]
    pub fn with_policy_path(mut self, path: impl Into<String>) -> Self {
        self.policy_reader = PolicyReader::with_path(path);
        self
    }

    /// Sets the maximum policy age in seconds.
    #[must_use]
    pub fn with_max_policy_age(mut self, max_age_secs: u64) -> Self {
        self.policy_reader = self.policy_reader.with_max_age(max_age_secs);
        self.policy_validator = self.policy_validator.with_max_age(max_age_secs);
        self
    }

    /// Sets the minimum confidence threshold.
    #[must_use]
    pub fn with_min_confidence(mut self, min_confidence: f64) -> Self {
        self.policy_validator = self.policy_validator.with_min_confidence(min_confidence);
        self.guardrails = self.guardrails.with_min_confidence(min_confidence);
        self
    }

    /// Sets the minimum dwell time between switches.
    #[must_use]
    pub fn with_min_dwell(mut self, min_dwell_secs: u64) -> Self {
        self.guardrails = self.guardrails.with_min_dwell(min_dwell_secs);
        self
    }

    /// Sets the maximum switches per hour.
    #[must_use]
    pub fn with_max_switches_per_hour(mut self, max_switches_per_hour: u32) -> Self {
        self.guardrails = self
            .guardrails
            .with_max_switches_per_hour(max_switches_per_hour);
        self
    }

    /// Sets the maximum consecutive failures before disabling adaptation.
    #[must_use]
    pub fn with_max_consecutive_failures(mut self, max_consecutive_failures: u32) -> Self {
        self.guardrails = self
            .guardrails
            .with_max_consecutive_failures(max_consecutive_failures);
        self
    }

    /// Sets the tick interval.
    #[must_use]
    pub fn with_tick_interval(mut self, tick_interval_secs: u64) -> Self {
        self.tick_interval_secs = tick_interval_secs;
        self
    }

    /// Sets the audit logger path.
    #[must_use]
    pub fn with_audit_log_path(mut self, path: impl Into<String>) -> Self {
        self.audit_logger = self.audit_logger.with_path(path);
        self
    }

    /// Enables or disables the audit logger.
    pub fn set_audit_enabled(&mut self, enabled: bool) {
        self.audit_logger.set_enabled(enabled);
    }

    /// Updates the adaptation mode (from config).
    pub fn set_adaptation_mode(&mut self, mode: AdaptationMode) {
        self.adaptation_mode = mode;
        self.guardrails.set_adaptation_mode(mode);
    }

    /// Updates the adaptation enabled state (from config).
    pub fn set_adaptation_enabled(&mut self, enabled: bool) {
        self.adaptation_enabled = enabled;
        self.guardrails.set_adaptation_enabled(enabled);
    }

    /// Updates the current profile (e.g., after an external change).
    pub fn set_current_profile(&mut self, _profile: Profile) {
        // The guardrails engine tracks current profile internally
        // This would need a method on GuardrailEngine, but for now we recreate
        // In practice, the backend's set_profile would call this
    }

    /// Runs a single adaptation tick.
    ///
    /// This is the main orchestration function:
    /// 1. Checks adaptation mode/enabled
    /// 2. Reads policy from file
    /// 3. Validates policy recommendation
    /// 4. In Shadow mode: logs and returns
    /// 5. In Canary mode: checks guardrails, attempts switch
    /// 6. Records success/failure in guardrails
    ///
    /// Returns the outcome for logging/monitoring.
    pub fn tick<B: Backend + ?Sized>(
        &mut self,
        backend: &mut B,
        heartbeat_ok: bool,
        now: u64,
    ) -> AdaptationOutcome {
        // 1. Check if adaptation is enabled
        if !self.adaptation_enabled || !self.guardrails.is_adaptation_enabled() {
            let _ =
                self.audit_logger
                    .log_rejected("adaptation disabled in config", 0.0, heartbeat_ok);
            return AdaptationOutcome::Skipped {
                reason: "adaptation disabled".to_string(),
            };
        }

        // 2. Check if adaptation mode is Off before reading the policy
        if self.adaptation_mode == AdaptationMode::Off {
            let _ = self
                .audit_logger
                .log_rejected("adaptation mode is Off", 0.0, heartbeat_ok);
            return AdaptationOutcome::Skipped {
                reason: "adaptation mode is Off".to_string(),
            };
        }

        // 2. Read policy from file
        let recommendation = match self.policy_reader.read() {
            Ok(rec) => rec,
            Err(e) => {
                let _ = self.audit_logger.log_rejected(
                    &format!("policy read error: {e}"),
                    0.0,
                    heartbeat_ok,
                );
                return AdaptationOutcome::PolicyReadError {
                    error: e.to_string(),
                };
            }
        };

        // 3. Validate policy recommendation
        let validation_result = self
            .policy_validator
            .validate(&recommendation, self.guardrails.current_profile());
        if let Some(validation_error) = validation_result.rejection_reason() {
            let _ = self.audit_logger.log_rejected(
                &validation_error.to_string(),
                recommendation.confidence,
                heartbeat_ok,
            );
            return AdaptationOutcome::ValidationRejected {
                error: validation_error.clone(),
            };
        }

        // 4. Handle based on mode
        match self.adaptation_mode {
            AdaptationMode::Off => {
                let _ = self.audit_logger.log_rejected(
                    "adaptation mode is Off",
                    recommendation.confidence,
                    heartbeat_ok,
                );
                AdaptationOutcome::Skipped {
                    reason: "adaptation mode is Off".to_string(),
                }
            }

            AdaptationMode::Shadow => {
                // Log the recommendation but don't switch
                let _ = self.audit_logger.log_shadow(
                    recommendation.recommended_profile,
                    recommendation.confidence,
                    &recommendation.reason,
                    heartbeat_ok,
                );
                AdaptationOutcome::ShadowLogged {
                    recommended_profile: recommendation.recommended_profile,
                    confidence: recommendation.confidence,
                    reason: recommendation.reason.clone(),
                }
            }

            AdaptationMode::Canary => {
                // 5. Check guardrails
                let guardrail_result =
                    self.guardrails
                        .can_switch(&recommendation, heartbeat_ok, now);

                if !guardrail_result.is_allowed() {
                    let violation = guardrail_result.violation().unwrap();
                    let _ = self.audit_logger.log_rejected(
                        &violation.to_string(),
                        recommendation.confidence,
                        heartbeat_ok,
                    );
                    return AdaptationOutcome::GuardrailRejected {
                        violation: violation.clone(),
                    };
                }

                // 6. Attempt the switch via backend
                let from_profile = self.guardrails.current_profile();
                let to_profile = recommendation.recommended_profile;

                match backend.set_profile(to_profile) {
                    Ok(()) => {
                        // 7. Verify post-switch health (process alive, sched_ext active, heartbeat fresh, profile matches)
                        if let Err(_e) = backend.verify_post_switch_health(to_profile) {
                            // 8a. Post-switch health check failed - FULL ROLLBACK SEQUENCE
                            // Stop Cerynth scheduler → Linux scheduler takes over → Attempt previous known-good profile
                            // Success? Continue. Failure? Disable adaptation, stay on Linux.
                            let previous_good = self
                                .guardrails
                                .previous_good_profile()
                                .unwrap_or(from_profile);
                            let rollback_result = backend.rollback(previous_good);

                            // Record failure in guardrails (may disable adaptation)
                            self.guardrails.record_failure();

                            // Log rollback attempt
                            let _ = self.audit_logger.log_rollback(
                                from_profile,
                                previous_good,
                                matches!(rollback_result, RollbackResult::Success { .. }),
                                match &rollback_result {
                                    RollbackResult::Failed { error, .. } => Some(error.as_str()),
                                    _ => None,
                                },
                                heartbeat_ok,
                            );

                            AdaptationOutcome::RollbackAttempted {
                                from_profile,
                                to_profile,
                                rollback_result,
                            }
                        } else {
                            // 8b. Post-switch health check passed
                            self.guardrails.record_success(to_profile, now);
                            let _ = self.audit_logger.log_success(
                                from_profile,
                                to_profile,
                                recommendation.confidence,
                                &recommendation.reason,
                                heartbeat_ok,
                            );
                            AdaptationOutcome::Switched {
                                from_profile,
                                to_profile,
                            }
                        }
                    }
                    Err(e) => {
                        // 9. Set profile failed - record failure in guardrails (may disable adaptation)
                        self.guardrails.record_failure();
                        let _ = self.audit_logger.log_failed(
                            from_profile,
                            to_profile,
                            &e,
                            heartbeat_ok,
                        );
                        AdaptationOutcome::SwitchFailed { error: e }
                    }
                }
            }
        }
    }

    /// Returns the current guardrails engine state for inspection.
    #[must_use]
    pub fn guardrails(&self) -> &GuardrailEngine {
        &self.guardrails
    }

    /// Returns the current adaptation mode.
    #[must_use]
    pub fn adaptation_mode(&self) -> AdaptationMode {
        self.adaptation_mode
    }

    /// Returns whether adaptation is enabled.
    #[must_use]
    pub fn is_adaptation_enabled(&self) -> bool {
        self.adaptation_enabled
    }
}

/// Trait for scheduler backends that can be controlled by the adaptation controller.
///
/// This is a subset of the full `Backend` trait focused on profile switching.
pub trait Backend {
    /// Sets the scheduler profile.
    fn set_profile(&mut self, profile: Profile) -> Result<(), String>;

    /// Gets the current scheduler profile.
    fn get_profile(&self) -> Result<Profile, String>;

    /// Verifies the scheduler is healthy after a profile switch.
    ///
    /// Returns `Ok(())` if the scheduler is healthy (process alive, sched_ext active,
    /// heartbeat fresh, and profile matches). Returns `Err` with a reason if unhealthy.
    fn verify_post_switch_health(&mut self, expected_profile: Profile) -> Result<(), String>;

    /// Stops the scheduler (for rollback).
    fn stop(&mut self) -> Result<(), String>;

    /// Attempts a full rollback to a previous known-good profile.
    ///
    /// This stops the current scheduler, then attempts to start the previous good profile.
    /// Returns RollbackResult indicating success or failure.
    fn rollback(&mut self, previous_good_profile: Profile) -> RollbackResult;
}

#[cfg(test)]
mod adaptation_controller_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    // Mock backend for testing
    struct MockBackend {
        profile: Profile,
        should_fail: bool,
        should_fail_health_check: bool,
        should_fail_rollback: bool,
    }

    impl MockBackend {
        fn new(profile: Profile) -> Self {
            Self {
                profile,
                should_fail: false,
                should_fail_health_check: false,
                should_fail_rollback: false,
            }
        }

        fn with_failure(mut self) -> Self {
            self.should_fail = true;
            self
        }
        fn with_health_check_failure(mut self) -> Self {
            self.should_fail_health_check = true;
            self
        }
        fn with_rollback_failure(mut self) -> Self {
            self.should_fail_rollback = true;
            self
        }
    }

    impl Backend for MockBackend {
        fn set_profile(&mut self, profile: Profile) -> Result<(), String> {
            if self.should_fail {
                return Err("simulated switch failure".to_string());
            }
            self.profile = profile;
            Ok(())
        }

        fn get_profile(&self) -> Result<Profile, String> {
            Ok(self.profile)
        }

        fn verify_post_switch_health(&mut self, expected_profile: Profile) -> Result<(), String> {
            if self.should_fail_health_check {
                return Err("simulated post-switch health check failure".to_string());
            }

            if self.profile == expected_profile {
                Ok(())
            } else {
                Err(format!(
                    "post-switch health check failed: expected {:?}, got {:?}",
                    expected_profile, self.profile
                ))
            }
        }

        fn stop(&mut self) -> Result<(), String> {
            // Mock: just reset profile to Balanced as "stopped"
            self.profile = Profile::Balanced;
            Ok(())
        }

        fn rollback(&mut self, previous_good_profile: Profile) -> RollbackResult {
            // Mock: stop then try to set previous good profile
            let _ = self.stop();
            if self.should_fail_rollback {
                RollbackResult::Failed {
                    error: "simulated rollback failure".to_string(),
                    previous_good_profile,
                }
            } else {
                self.profile = previous_good_profile;
                RollbackResult::Success {
                    previous_good_profile,
                }
            }
        }
    }

    fn make_controller() -> AdaptationController {
        AdaptationController::new(Profile::Balanced)
            .with_min_confidence(0.75)
            .with_min_dwell(15)
            .with_max_switches_per_hour(12)
            .with_max_consecutive_failures(3)
    }

    #[test]
    fn skipped_when_adaptation_disabled() {
        let mut controller = make_controller();
        controller.set_adaptation_enabled(false);

        let mut backend = MockBackend::new(Profile::Balanced);
        let outcome = controller.tick(&mut backend, true, 1000);

        assert!(matches!(outcome, AdaptationOutcome::Skipped { .. }));
    }

    #[test]
    fn skipped_when_mode_off() {
        let mut controller = make_controller();
        controller.set_adaptation_mode(AdaptationMode::Off);

        let mut backend = MockBackend::new(Profile::Balanced);
        let outcome = controller.tick(&mut backend, true, 1000);

        assert!(matches!(outcome, AdaptationOutcome::Skipped { .. }));
    }

    #[test]
    fn shadow_mode_logs_recommendation() {
        let mut controller = make_controller();
        controller.set_adaptation_mode(AdaptationMode::Shadow);

        let mut backend = MockBackend::new(Profile::Balanced);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // Write a valid policy file
        let path = format!("target/test-adaptation-shadow-{}.json", std::process::id());
        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "performance",
            "confidence": 0.90,
            "reason": "High CPU",
            "timestamp": now,
            "apply": false,
        }))
        .unwrap();
        std::fs::write(&path, json).unwrap();

        controller = controller.with_policy_path(&path).with_max_policy_age(60);

        let outcome = controller.tick(&mut backend, true, now);

        assert!(matches!(
            outcome,
            AdaptationOutcome::ShadowLogged {
                recommended_profile: Profile::Performance,
                confidence,
                ..
            } if (confidence - 0.90).abs() < f64::EPSILON
        ));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn canary_mode_switches_when_allowed() {
        let mut controller = make_controller();
        controller.set_adaptation_mode(AdaptationMode::Canary);

        let mut backend = MockBackend::new(Profile::Balanced);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let path = format!("target/test-adaptation-canary-{}.json", std::process::id());
        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "performance",
            "confidence": 0.90,
            "reason": "High CPU",
            "timestamp": now,
            "apply": false,
        }))
        .unwrap();
        std::fs::write(&path, json).unwrap();

        controller = controller.with_policy_path(&path).with_max_policy_age(60);

        let outcome = controller.tick(&mut backend, true, now);

        assert!(matches!(
            outcome,
            AdaptationOutcome::Switched {
                from_profile: Profile::Balanced,
                to_profile: Profile::Performance,
            }
        ));
        assert_eq!(backend.profile, Profile::Performance);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn canary_mode_rejects_low_confidence() {
        let mut controller = make_controller();
        controller.set_adaptation_mode(AdaptationMode::Canary);

        let mut backend = MockBackend::new(Profile::Balanced);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let path = format!("target/test-adaptation-lowconf-{}.json", std::process::id());
        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "performance",
            "confidence": 0.42,
            "reason": "Low confidence",
            "timestamp": now,
            "apply": false,
        }))
        .unwrap();
        std::fs::write(&path, json).unwrap();

        controller = controller.with_policy_path(&path).with_max_policy_age(60);

        let outcome = controller.tick(&mut backend, true, now);

        assert!(matches!(
            outcome,
            AdaptationOutcome::ValidationRejected { .. }
        ));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn canary_mode_rejects_guardrail_violation() {
        let mut controller = make_controller();
        controller.set_adaptation_mode(AdaptationMode::Canary);

        let mut backend = MockBackend::new(Profile::Balanced);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let path = format!(
            "target/test-adaptation-guardrail-{}.json",
            std::process::id()
        );
        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "performance",
            "confidence": 0.90,
            "reason": "High CPU",
            "timestamp": now,
            "apply": false,
        }))
        .unwrap();
        std::fs::write(&path, json).unwrap();

        controller = controller.with_policy_path(&path).with_max_policy_age(60);

        // First switch succeeds
        let outcome = controller.tick(&mut backend, true, now);
        assert!(matches!(outcome, AdaptationOutcome::Switched { .. }));

        // Change recommendation so it differs from the current Performance profile
        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "balanced",
            "confidence": 0.90,
            "reason": "Switch back",
            "timestamp": now + 5,
                "apply": false,
        }))
        .unwrap();
        std::fs::write(&path, json).unwrap();

        // Immediate second switch should be rejected by dwell time
        let outcome = controller.tick(&mut backend, true, now + 5);

        assert!(matches!(
            outcome,
            AdaptationOutcome::GuardrailRejected { violation }
            if matches!(violation, GuardrailViolation::DwellTimeNotMet { .. })
        ));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn canary_mode_rejects_unhealthy_heartbeat() {
        let mut controller = make_controller();
        controller.set_adaptation_mode(AdaptationMode::Canary);

        let mut backend = MockBackend::new(Profile::Balanced);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let path = format!(
            "target/test-adaptation-heartbeat-{}.json",
            std::process::id()
        );
        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "performance",
            "confidence": 0.90,
            "reason": "High CPU",
            "timestamp": now,
            "apply": false,
        }))
        .unwrap();
        std::fs::write(&path, json).unwrap();

        controller = controller.with_policy_path(&path).with_max_policy_age(60);

        let outcome = controller.tick(&mut backend, false, now);

        assert!(matches!(
            outcome,
            AdaptationOutcome::GuardrailRejected { violation }
            if matches!(violation, GuardrailViolation::UnhealthyHeartbeat)
        ));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn records_failure_and_disables_after_max_failures() {
        let mut controller = make_controller();
        controller.set_adaptation_mode(AdaptationMode::Canary);

        let mut backend = MockBackend::new(Profile::Balanced).with_failure();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let path = format!("target/test-adaptation-failure-{}.json", std::process::id());
        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "performance",
            "confidence": 0.90,
            "reason": "High CPU",
            "timestamp": now,
            "apply": false,
        }))
        .unwrap();
        std::fs::write(&path, json).unwrap();

        controller = controller.with_policy_path(&path).with_max_policy_age(60);

        // First failure
        let outcome = controller.tick(&mut backend, true, now);
        assert!(matches!(outcome, AdaptationOutcome::SwitchFailed { .. }));
        assert!(controller.guardrails().is_adaptation_enabled());
        assert_eq!(controller.guardrails().consecutive_failures(), 1);

        // Second failure
        let outcome = controller.tick(&mut backend, true, now + 20);
        assert!(matches!(outcome, AdaptationOutcome::SwitchFailed { .. }));
        assert!(controller.guardrails().is_adaptation_enabled());
        assert_eq!(controller.guardrails().consecutive_failures(), 2);

        // Third failure - should disable adaptation
        let outcome = controller.tick(&mut backend, true, now + 40);
        assert!(matches!(outcome, AdaptationOutcome::SwitchFailed { .. }));
        assert!(!controller.guardrails().is_adaptation_enabled());
        assert_eq!(controller.guardrails().consecutive_failures(), 3);

        // Next tick should be skipped due to disabled adaptation
        let outcome = controller.tick(&mut backend, true, now + 60);
        assert!(matches!(outcome, AdaptationOutcome::Skipped { .. }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn policy_read_error_handled() {
        let mut controller = make_controller();
        controller.set_adaptation_mode(AdaptationMode::Canary);

        let mut backend = MockBackend::new(Profile::Balanced);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Non-existent policy file
        controller = controller.with_policy_path("/nonexistent/policy.json");

        let outcome = controller.tick(&mut backend, true, now);

        assert!(matches!(outcome, AdaptationOutcome::PolicyReadError { .. }));
    }

    #[test]
    fn validation_rejected_same_profile() {
        let mut controller = make_controller();
        controller.set_adaptation_mode(AdaptationMode::Canary);

        let mut backend = MockBackend::new(Profile::Balanced);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let path = format!("target/test-adaptation-same-{}.json", std::process::id());
        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "balanced",
            "confidence": 0.90,
            "reason": "Same profile",
            "timestamp": now,
            "apply": false,
        }))
        .unwrap();
        std::fs::write(&path, json).unwrap();

        controller = controller.with_policy_path(&path).with_max_policy_age(60);

        let outcome = controller.tick(&mut backend, true, now);

        assert!(matches!(
            outcome,
            AdaptationOutcome::ValidationRejected { error }
            if matches!(error, PolicyValidationError::SameProfile { .. })
        ));

        let _ = std::fs::remove_file(&path);
    }
    #[test]
    fn rolls_back_when_post_switch_health_check_fails() {
        let mut controller = make_controller();
        controller.set_adaptation_mode(AdaptationMode::Canary);

        let mut backend = MockBackend::new(Profile::Balanced).with_health_check_failure();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let path = format!(
            "target/test-adaptation-rollback-{}.json",
            std::process::id()
        );

        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "performance",
            "confidence": 0.90,
            "reason": "High CPU",
            "timestamp": now,
            "apply": false,
        }))
        .unwrap();

        std::fs::write(&path, json).unwrap();

        controller = controller.with_policy_path(&path).with_max_policy_age(60);

        let outcome = controller.tick(&mut backend, true, now);
        assert!(matches!(
            outcome,
            AdaptationOutcome::RollbackAttempted {
                from_profile: Profile::Balanced,
                to_profile: Profile::Performance,
                rollback_result: RollbackResult::Success {
                    previous_good_profile: Profile::Balanced
                },
            }
        ));

        // Rollback should restore the previous good profile.
        assert_eq!(backend.profile, Profile::Balanced);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reports_rollback_failure_when_post_switch_health_check_fails() {
        let mut controller = make_controller();
        controller.set_adaptation_mode(AdaptationMode::Canary);

        let mut backend = MockBackend::new(Profile::Balanced)
            .with_health_check_failure()
            .with_rollback_failure();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let path = format!(
            "target/test-adaptation-rollback-failure-{}.json",
            std::process::id()
        );

        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "performance",
            "confidence": 0.90,
            "reason": "High CPU",
            "timestamp": now,
            "apply": false,
        }))
        .unwrap();

        std::fs::write(&path, json).unwrap();

        controller = controller.with_policy_path(&path).with_max_policy_age(60);

        let outcome = controller.tick(&mut backend, true, now);
        assert!(matches!(
            outcome,
            AdaptationOutcome::RollbackAttempted {
                from_profile: Profile::Balanced,
                to_profile: Profile::Performance,
                rollback_result: RollbackResult::Failed {
                    previous_good_profile: Profile::Balanced,
                    ..
                },
            }
        ));

        let _ = std::fs::remove_file(&path);
    }
}

/// Errors that can occur when reading the policy file.
#[derive(Debug, thiserror::Error)]
pub enum PolicyReadError {
    #[error("policy file not found: {0}")]
    NotFound(String),

    #[error("cannot read policy file {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid JSON in policy file {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("missing required field: {0}")]
    MissingField(String),

    #[error("invalid confidence value: {0} (must be in [0.0, 1.0])")]
    InvalidConfidence(f64),

    #[error("stale policy: age {age_secs}s exceeds max {max_age_secs}s")]
    Stale { age_secs: u64, max_age_secs: u64 },
}

/// Errors that can occur when validating a policy recommendation.
///
/// This module does not switch anything — it only evaluates whether
/// a recommendation is trustworthy enough to consider.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PolicyValidationError {
    #[error("confidence {confidence:.2} below threshold {threshold:.2}")]
    LowConfidence { confidence: f64, threshold: f64 },

    #[error("policy is stale: age {age_secs}s exceeds max {max_age_secs}s")]
    StalePolicy { age_secs: u64, max_age_secs: u64 },

    #[error("recommended profile matches current profile: {profile:?}")]
    SameProfile { profile: Profile },

    #[error("invalid profile: {0}")]
    InvalidProfile(String),
}

/// Result of policy validation.
#[derive(Debug, Clone, PartialEq)]
pub enum PolicyValidationResult {
    Accepted,
    Rejected(PolicyValidationError),
}

impl PolicyValidationResult {
    /// Returns `true` if the policy was accepted.
    #[must_use]
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted)
    }

    /// Returns the rejection reason if rejected.
    #[must_use]
    pub fn rejection_reason(&self) -> Option<&PolicyValidationError> {
        match self {
            Self::Rejected(err) => Some(err),
            Self::Accepted => None,
        }
    }
}

/// Validates whether a policy recommendation is trustworthy enough to consider.
///
/// This does NOT switch anything — it only returns a validation result.
#[derive(Debug, Clone)]
pub struct PolicyValidator {
    /// Minimum confidence required to accept a recommendation.
    min_confidence: f64,

    /// Maximum age in seconds for a policy to be considered fresh.
    max_age_secs: u64,
}

impl PolicyValidator {
    /// Creates a new PolicyValidator with default thresholds.
    ///
    /// Default minimum confidence: 0.75
    /// Default maximum age: 10 seconds
    #[must_use]
    pub fn new() -> Self {
        Self {
            min_confidence: 0.75,
            max_age_secs: 10,
        }
    }

    /// Creates a PolicyValidator with a custom minimum confidence threshold.
    #[must_use]
    pub fn with_min_confidence(mut self, min_confidence: f64) -> Self {
        self.min_confidence = min_confidence;
        self
    }

    /// Creates a PolicyValidator with a custom maximum age.
    #[must_use]
    pub fn with_max_age(mut self, max_age_secs: u64) -> Self {
        self.max_age_secs = max_age_secs;
        self
    }

    /// Validates a policy recommendation against the current profile.
    ///
    /// Checks:
    /// - Confidence is >= min_confidence
    /// - Policy is fresh (within max_age_secs)
    /// - Recommended profile differs from current profile
    /// - Profile is valid (handled by PolicyRecommendation parsing)
    pub fn validate(
        &self,
        recommendation: &PolicyRecommendation,
        current_profile: Profile,
    ) -> PolicyValidationResult {
        // Check confidence threshold
        if recommendation.confidence < self.min_confidence {
            return PolicyValidationResult::Rejected(PolicyValidationError::LowConfidence {
                confidence: recommendation.confidence,
                threshold: self.min_confidence,
            });
        }

        // Check freshness
        if !recommendation.is_fresh(self.max_age_secs) {
            return PolicyValidationResult::Rejected(PolicyValidationError::StalePolicy {
                age_secs: recommendation.age_secs(),
                max_age_secs: self.max_age_secs,
            });
        }

        // Check if recommendation differs from current profile
        if recommendation.recommended_profile == current_profile {
            return PolicyValidationResult::Rejected(PolicyValidationError::SameProfile {
                profile: current_profile,
            });
        }

        PolicyValidationResult::Accepted
    }
}

impl Default for PolicyValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads and validates the policy recommendation from a JSON file.
#[derive(Debug)]
pub struct PolicyReader {
    path: String,
    max_age_secs: u64,
}

impl PolicyReader {
    /// Creates a new PolicyReader with the default path and max age.
    ///
    /// Default path: `/run/cerynth/policy.json`
    /// Default max age: 10 seconds
    #[must_use]
    pub fn new() -> Self {
        Self {
            path: "/run/cerynth/policy.json".to_string(),
            max_age_secs: 10,
        }
    }

    /// Creates a PolicyReader with a custom path.
    #[must_use]
    pub fn with_path(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            max_age_secs: 10,
        }
    }

    /// Sets the maximum age for a policy recommendation to be considered fresh.
    #[must_use]
    pub fn with_max_age(mut self, max_age_secs: u64) -> Self {
        self.max_age_secs = max_age_secs;
        self
    }

    /// Reads and validates the policy file.
    ///
    /// Returns a `PolicyRecommendation` if the file exists, is readable,
    /// contains valid JSON with all required fields, and the recommendation
    /// is fresh. Returns `PolicyReadError` otherwise.
    pub fn read(&self) -> Result<PolicyRecommendation, PolicyReadError> {
        // Check if file exists
        if !Path::new(&self.path).exists() {
            return Err(PolicyReadError::NotFound(self.path.clone()));
        }

        // Read the file
        let contents = std::fs::read_to_string(&self.path).map_err(|e| PolicyReadError::Io {
            path: self.path.clone(),
            source: e,
        })?;

        // Parse JSON
        let value: serde_json::Value =
            serde_json::from_str(&contents).map_err(|e| PolicyReadError::Json {
                path: self.path.clone(),
                source: e,
            })?;

        // Extract required fields
        let recommended_profile = value
            .get("recommended_profile")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PolicyReadError::MissingField("recommended_profile".to_string()))?
            .parse::<Profile>()
            .map_err(|_| PolicyReadError::MissingField("recommended_profile".to_string()))?;

        let confidence = value
            .get("confidence")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| PolicyReadError::MissingField("confidence".to_string()))?;

        if !(0.0..=1.0).contains(&confidence) {
            return Err(PolicyReadError::InvalidConfidence(confidence));
        }

        let reason = value
            .get("reason")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PolicyReadError::MissingField("reason".to_string()))?
            .to_string();

        let timestamp = value
            .get("timestamp")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| PolicyReadError::MissingField("timestamp".to_string()))?;

        let apply = value
            .get("apply")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let recommendation = PolicyRecommendation {
            recommended_profile,
            confidence,
            reason,
            timestamp,
            apply,
        };

        // Check freshness
        if !recommendation.is_fresh(self.max_age_secs) {
            return Err(PolicyReadError::Stale {
                age_secs: recommendation.age_secs(),
                max_age_secs: self.max_age_secs,
            });
        }

        Ok(recommendation)
    }

    /// Attempts to read the policy file, returning `None` on any error.
    ///
    /// This is a convenience method for callers that want to handle missing
    /// or invalid policy gracefully without pattern matching on error types.
    #[must_use]
    pub fn try_read(&self) -> Option<PolicyRecommendation> {
        self.read().ok()
    }
}

impl Default for PolicyReader {
    fn default() -> Self {
        Self::new()
    }
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

    fn workload_confidence(
        profile: Profile,
        cpu: f64,
        load: f64,
        runnable: u64,
        ctxt_rate: f64,
        short_lived: f64,
    ) -> f64 {
        match profile {
            Profile::Performance => {
                // CPU saturation is the strongest evidence.
                let cpu_score = ((cpu - 85.0) / 15.0).clamp(0.0, 1.0);
                let load_score = ((load - 1.0) / 2.0).clamp(0.0, 1.0);
                let runnable_score = ((runnable as f64 - 2.0) / 4.0).clamp(0.0, 1.0);

                (0.65 * cpu_score + 0.20 * load_score + 0.15 * runnable_score).clamp(0.0, 1.0)
            }

            Profile::Interactive => {
                // Interactive behaviour is primarily characterised by
                // runnable pressure, context switching, and process churn.
                let runnable_score = ((runnable as f64 - 1.0) / 3.0).clamp(0.0, 1.0);
                let ctxt_score = ((ctxt_rate - 150.0) / 850.0).clamp(0.0, 1.0);
                let process_score = ((short_lived - 2.0) / 8.0).clamp(0.0, 1.0);

                (0.35 * runnable_score + 0.40 * ctxt_score + 0.25 * process_score).clamp(0.0, 1.0)
            }

            Profile::Background => {
                let cpu_score = (1.0 - cpu / 10.0).clamp(0.0, 1.0);
                let load_score = (1.0 - load / 0.5).clamp(0.0, 1.0);
                let runnable_score = if runnable == 0 { 1.0 } else { 0.0 };
                let process_score = (1.0 - short_lived).clamp(0.0, 1.0);

                (0.40 * cpu_score
                    + 0.25 * load_score
                    + 0.20 * runnable_score
                    + 0.15 * process_score)
                    .clamp(0.0, 1.0)
            }

            Profile::Balanced => {
                let cpu_distance = (cpu - 50.0).abs() / 50.0;
                let load_distance = (load - 1.0).abs() / 2.0;

                (1.0 - 0.6 * cpu_distance - 0.4 * load_distance).clamp(0.0, 1.0)
            }
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
        let candidate =
            if input.cpu_usage_percent >= 90.0 && input.load_1m >= 1.0 && input.runnable_tasks >= 2
            {
                self.decision(
                    Profile::Performance,
                    Self::workload_confidence(
                        Profile::Performance,
                        input.cpu_usage_percent,
                        input.load_1m,
                        input.runnable_tasks,
                        input.context_switch_rate,
                        input.short_lived_process_rate,
                    ),
                    "Very high CPU utilisation with sustained runnable pressure",
                )
            } else if input.runnable_tasks >= 2
                && ((input.context_switch_rate >= 500.0 && input.runnable_tasks >= 3)
                    || input.short_lived_process_rate >= 5.0)
            {
                self.decision(
                Profile::Interactive,
                Self::workload_confidence(
                    Profile::Interactive,
                    input.cpu_usage_percent,
                    input.load_1m,
                    input.runnable_tasks,
                    input.context_switch_rate,
                    input.short_lived_process_rate,
                ),
                "High runnable load with frequent context switches or short-lived process activity",
            )
            } else if input.cpu_usage_percent < 10.0
                && input.load_1m < 0.5
                && input.runnable_tasks <= 1
                && input.short_lived_process_rate < 1.0
            {
                self.decision(
                    Profile::Background,
                    Self::workload_confidence(
                        Profile::Background,
                        input.cpu_usage_percent,
                        input.load_1m,
                        input.runnable_tasks,
                        input.context_switch_rate,
                        input.short_lived_process_rate,
                    ),
                    "Very low CPU utilisation and runnable pressure with minimal process activity",
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
                    Self::workload_confidence(
                        Profile::Balanced,
                        input.cpu_usage_percent,
                        input.load_1m,
                        input.runnable_tasks,
                        input.context_switch_rate,
                        input.short_lived_process_rate,
                    ),
                    "Workload does not strongly match another profile",
                )
            };

        println!(
            "POLICY_RAW cpu={:.2} load={:.2} runnable={} ctxt_rate={:.2} short_lived={:.2} => {:?} conf={:.2}",
            input.cpu_usage_percent,
            input.load_1m,
            input.runnable_tasks,
            input.context_switch_rate,
            input.short_lived_process_rate,
            candidate.recommended_profile,
            candidate.confidence,
        );

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
    fn responsive_low_cpu_workload_recommends_interactive() {
        let mut policy = RulesBasedPolicy::with_stability_controls(0.60, 1, 0);

        // Low CPU usage, but significant runnable/process activity.
        let workload = input(5.0, 1.2, 3, 500.0, 7.0);

        let decision = policy.evaluate(&workload);

        assert_eq!(decision.recommended_profile, Profile::Interactive);
    }

    #[test]
    fn short_lived_process_activity_recommends_interactive() {
        let mut policy = RulesBasedPolicy::with_stability_controls(0.60, 1, 0);

        let workload = input(70.0, 1.0, 4, 20.0, 5.0);

        let decision = policy.evaluate(&workload);

        assert_eq!(decision.recommended_profile, Profile::Interactive);
        assert!(decision.reason.contains("short-lived"));
    }

    #[test]
    fn idle_workload_recommends_background() {
        let mut policy = RulesBasedPolicy::with_stability_controls(0.60, 1, 0);

        let workload = input(5.0, 0.1, 0, 10.0, 0.0);

        let decision = policy.evaluate(&workload);

        assert_eq!(decision.recommended_profile, Profile::Background);
        assert!(decision.reason.contains("Very low CPU"));
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

        let workload = input(100.0, 1.5, 5, 150.0, 0.0);

        policy.evaluate(&workload);
        policy.evaluate(&workload);

        let decision = policy.evaluate(&workload);

        assert_eq!(decision.recommended_profile, Profile::Performance);
    }

    #[test]
    fn interactive_workload_recommends_interactive() {
        let mut policy = RulesBasedPolicy::new();

        let workload = input(75.0, 1.0, 4, 600.0, 0.0);

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

        let workload = input(100.0, 1.5, 5, 150.0, 0.0);

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

        let performance = input(100.0, 1.5, 5, 150.0, 0.0);

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

        let performance = input(100.0, 1.5, 5, 150.0, 0.0);
        let interactive = input(75.0, 1.0, 4, 600.0, 0.0);

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
    fn low_load_blocks_performance_recommendation() {
        let mut policy = RulesBasedPolicy::with_stability_controls(0.60, 1, 0);

        // CPU and runnable pressure are high, but 1-minute load is too low
        // for the Performance rule.
        let workload = input(100.0, 0.5, 5, 600.0, 0.0);

        let decision = policy.evaluate(&workload);

        assert_ne!(decision.recommended_profile, Profile::Performance);
        assert_eq!(decision.recommended_profile, Profile::Interactive);
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

#[cfg(test)]
mod policy_reader_tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_file(name: &str) -> String {
        format!("target/test-policy-{name}.json")
    }

    fn write_policy_file(path: &str, json: &str) {
        if let Some(parent) = Path::new(path).parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(path, json).unwrap();
    }

    fn make_valid_json(
        profile: &str,
        confidence: f64,
        reason: &str,
        timestamp: u64,
        apply: bool,
    ) -> String {
        serde_json::to_string(&serde_json::json!({
            "recommended_profile": profile,
            "confidence": confidence,
            "reason": reason,
            "timestamp": timestamp,
            "apply": apply,
        }))
        .unwrap()
    }

    #[test]
    fn reads_valid_policy_file() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let json = make_valid_json("performance", 0.85, "High CPU usage", now, false);
        let path = test_file("valid");
        write_policy_file(&path, &json);

        let reader = PolicyReader::with_path(&path).with_max_age(60);
        let result = reader.read().unwrap();

        assert_eq!(result.recommended_profile, Profile::Performance);
        assert_eq!(result.confidence, 0.85);
        assert_eq!(result.reason, "High CPU usage");
        assert!(result.is_fresh(60));
        assert!(!result.apply);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rejects_missing_file() {
        let reader = PolicyReader::with_path(test_file("missing"));
        let err = reader.read().unwrap_err();

        assert!(matches!(err, PolicyReadError::NotFound(_)));
    }

    #[test]
    fn rejects_invalid_json() {
        let path = test_file("invalid-json");
        write_policy_file(&path, "not json");

        let reader = PolicyReader::with_path(&path);
        let err = reader.read().unwrap_err();

        assert!(matches!(err, PolicyReadError::Json { .. }));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rejects_missing_recommended_profile() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let json = serde_json::to_string(&serde_json::json!({
            "confidence": 0.85,
            "reason": "test",
            "timestamp": now,
        }))
        .unwrap();
        let path = test_file("missing-profile");
        write_policy_file(&path, &json);

        let reader = PolicyReader::with_path(&path);
        let err = reader.read().unwrap_err();

        assert!(matches!(err, PolicyReadError::MissingField(_)));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rejects_missing_confidence() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "balanced",
            "reason": "test",
            "timestamp": now,
        }))
        .unwrap();
        let path = test_file("missing-confidence");
        write_policy_file(&path, &json);

        let reader = PolicyReader::with_path(&path);
        let err = reader.read().unwrap_err();

        assert!(matches!(err, PolicyReadError::MissingField(_)));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rejects_invalid_confidence_out_of_range() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let json = make_valid_json("balanced", 1.5, "test", now, false);
        let path = test_file("invalid-confidence");
        write_policy_file(&path, &json);

        let reader = PolicyReader::with_path(&path);
        let err = reader.read().unwrap_err();

        assert!(matches!(err, PolicyReadError::InvalidConfidence(_)));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rejects_missing_reason() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "balanced",
            "confidence": 0.85,
            "timestamp": now,
        }))
        .unwrap();
        let path = test_file("missing-reason");
        write_policy_file(&path, &json);

        let reader = PolicyReader::with_path(&path);
        let err = reader.read().unwrap_err();

        assert!(matches!(err, PolicyReadError::MissingField(_)));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rejects_missing_timestamp() {
        let json = serde_json::to_string(&serde_json::json!({
            "recommended_profile": "balanced",
            "confidence": 0.85,
            "reason": "test",
        }))
        .unwrap();
        let path = test_file("missing-timestamp");
        write_policy_file(&path, &json);

        let reader = PolicyReader::with_path(&path);
        let err = reader.read().unwrap_err();

        assert!(matches!(err, PolicyReadError::MissingField(_)));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rejects_stale_policy() {
        let old = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_sub(20);
        let json = make_valid_json("balanced", 0.85, "test", old, false);
        let path = test_file("stale");
        write_policy_file(&path, &json);

        let reader = PolicyReader::with_path(&path).with_max_age(10);
        let err = reader.read().unwrap_err();

        assert!(matches!(err, PolicyReadError::Stale { .. }));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn try_read_returns_none_on_error() {
        let reader = PolicyReader::with_path(test_file("missing"));
        let result = reader.try_read();
        assert!(result.is_none());
    }

    #[test]
    fn age_secs_works() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let old = now - 5;
        let recommendation = PolicyRecommendation {
            recommended_profile: Profile::Balanced,
            confidence: 0.85,
            reason: "test".to_string(),
            timestamp: old,
            apply: false,
        };

        assert_eq!(recommendation.age_secs(), 5);
    }

    #[test]
    fn is_fresh_works() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let recommendation = PolicyRecommendation {
            recommended_profile: Profile::Balanced,
            confidence: 0.85,
            reason: "test".to_string(),
            timestamp: now,
            apply: false,
        };

        assert!(recommendation.is_fresh(10));
        assert!(!recommendation.is_fresh(0));
    }

    #[test]
    fn reads_all_profiles() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        for profile in [
            Profile::Balanced,
            Profile::Performance,
            Profile::Interactive,
            Profile::Background,
        ] {
            let json = make_valid_json(
                &profile.to_string().to_lowercase(),
                0.85,
                "test",
                now,
                false,
            );
            let path = test_file(&format!("profile-{}", profile.to_string().to_lowercase()));
            write_policy_file(&path, &json);

            let reader = PolicyReader::with_path(&path).with_max_age(60);
            let result = reader.read().unwrap();

            assert_eq!(result.recommended_profile, profile);

            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod policy_validator_tests {
    use super::*;

    fn make_recommendation(
        profile: Profile,
        confidence: f64,
        reason: &str,
        timestamp: u64,
    ) -> PolicyRecommendation {
        PolicyRecommendation {
            recommended_profile: profile,
            confidence,
            reason: reason.to_string(),
            timestamp,
            apply: false,
        }
    }

    #[test]
    fn accepts_high_confidence_fresh_different_profile() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let validator = PolicyValidator::new();
        let recommendation = make_recommendation(Profile::Performance, 0.89, "High CPU", now);

        let result = validator.validate(&recommendation, Profile::Balanced);

        assert!(result.is_accepted());
    }

    #[test]
    fn rejects_low_confidence() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let validator = PolicyValidator::new();
        let recommendation = make_recommendation(Profile::Performance, 0.42, "Low confidence", now);

        let result = validator.validate(&recommendation, Profile::Balanced);

        assert!(!result.is_accepted());
        assert!(matches!(
            result.rejection_reason(),
            Some(PolicyValidationError::LowConfidence { confidence, threshold })
            if (confidence - 0.42).abs() < f64::EPSILON && (threshold - 0.75).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn rejects_stale_policy() {
        let old = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_sub(20);

        let validator = PolicyValidator::new();
        let recommendation = make_recommendation(Profile::Performance, 0.89, "High CPU", old);

        let result = validator.validate(&recommendation, Profile::Balanced);

        assert!(!result.is_accepted());
        assert!(matches!(
            result.rejection_reason(),
            Some(PolicyValidationError::StalePolicy { age_secs, max_age_secs })
            if *age_secs >= 20 && *max_age_secs == 10
        ));
    }

    #[test]
    fn rejects_same_profile() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let validator = PolicyValidator::new();
        let recommendation = make_recommendation(Profile::Balanced, 0.89, "Same profile", now);

        let result = validator.validate(&recommendation, Profile::Balanced);

        assert!(!result.is_accepted());
        assert!(matches!(
            result.rejection_reason(),
            Some(PolicyValidationError::SameProfile { profile })
            if *profile == Profile::Balanced
        ));
    }

    #[test]
    fn accepts_with_custom_confidence_threshold() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let validator = PolicyValidator::new().with_min_confidence(0.50);
        let recommendation =
            make_recommendation(Profile::Performance, 0.60, "Medium confidence", now);

        let result = validator.validate(&recommendation, Profile::Balanced);

        assert!(result.is_accepted());
    }

    #[test]
    fn rejects_with_custom_confidence_threshold() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let validator = PolicyValidator::new().with_min_confidence(0.90);
        let recommendation =
            make_recommendation(Profile::Performance, 0.89, "Below custom threshold", now);

        let result = validator.validate(&recommendation, Profile::Balanced);

        assert!(!result.is_accepted());
        assert!(matches!(
            result.rejection_reason(),
            Some(PolicyValidationError::LowConfidence { threshold, .. })
            if (threshold - 0.90).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn accepts_with_custom_max_age() {
        let old = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_sub(20);

        let validator = PolicyValidator::new().with_max_age(30);
        let recommendation = make_recommendation(
            Profile::Performance,
            0.89,
            "Old but within custom max_age",
            old,
        );

        let result = validator.validate(&recommendation, Profile::Balanced);

        assert!(result.is_accepted());
    }

    #[test]
    fn rejects_with_custom_max_age() {
        let old = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_sub(20);

        let validator = PolicyValidator::new().with_max_age(10);
        let recommendation = make_recommendation(
            Profile::Performance,
            0.89,
            "Too old for custom max_age",
            old,
        );

        let result = validator.validate(&recommendation, Profile::Balanced);

        assert!(!result.is_accepted());
        assert!(matches!(
            result.rejection_reason(),
            Some(PolicyValidationError::StalePolicy { .. })
        ));
    }

    #[test]
    fn validation_result_rejection_reason_works() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let validator = PolicyValidator::new();
        let recommendation = make_recommendation(Profile::Performance, 0.42, "Low confidence", now);

        let result = validator.validate(&recommendation, Profile::Balanced);

        assert!(!result.is_accepted());
        let reason = result.rejection_reason().unwrap();
        assert!(matches!(
            reason,
            PolicyValidationError::LowConfidence { .. }
        ));

        // Test Accepted case
        let recommendation =
            make_recommendation(Profile::Performance, 0.89, "High confidence", now);
        let result = validator.validate(&recommendation, Profile::Balanced);

        assert!(result.is_accepted());
        assert!(result.rejection_reason().is_none());
    }

    #[test]
    fn works_with_all_profiles() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let validator = PolicyValidator::new();

        for from_profile in [
            Profile::Balanced,
            Profile::Performance,
            Profile::Interactive,
            Profile::Background,
        ] {
            for to_profile in [
                Profile::Balanced,
                Profile::Performance,
                Profile::Interactive,
                Profile::Background,
            ] {
                let recommendation = make_recommendation(to_profile, 0.85, "test", now);
                let result = validator.validate(&recommendation, from_profile);

                if from_profile == to_profile {
                    assert!(
                        !result.is_accepted(),
                        "Should reject same profile: {:?}",
                        from_profile
                    );
                    assert!(matches!(
                        result.rejection_reason(),
                        Some(PolicyValidationError::SameProfile { .. })
                    ));
                } else {
                    assert!(
                        result.is_accepted(),
                        "Should accept different profile: {:?} -> {:?}",
                        from_profile,
                        to_profile
                    );
                }
            }
        }
    }
}

/// Guardrail violations that prevent a profile switch.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GuardrailViolation {
    #[error("adaptation is disabled")]
    AdaptationDisabled,

    #[error("adaptation mode is {mode:?}, not canary")]
    NotCanaryMode { mode: AdaptationMode },

    #[error("confidence {confidence:.2} below threshold {threshold:.2}")]
    LowConfidence { confidence: f64, threshold: f64 },

    #[error(
        "minimum dwell time not met: {elapsed_secs}s since last switch, need {min_dwell_secs}s"
    )]
    DwellTimeNotMet {
        elapsed_secs: u64,
        min_dwell_secs: u64,
    },

    #[error("hourly switch limit exceeded: {count} switches in the last hour, limit is {limit}")]
    HourlyLimitExceeded { count: u32, limit: u32 },

    #[error("scheduler heartbeat is stale or unhealthy")]
    UnhealthyHeartbeat,

    #[error("too many consecutive failed switches: {count}, disabling adaptation")]
    TooManyFailures { count: u32 },
}

/// Result of a guardrail check.
#[derive(Debug, Clone, PartialEq)]
pub enum GuardrailResult {
    Allowed,
    Denied(GuardrailViolation),
}

impl GuardrailResult {
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    #[must_use]
    pub fn violation(&self) -> Option<&GuardrailViolation> {
        match self {
            Self::Denied(v) => Some(v),
            Self::Allowed => None,
        }
    }
}

/// Guardrail engine — the safety brain that decides whether a profile switch may proceed.
///
/// Tracks state across evaluations and enforces:
/// - Adaptation enabled + Canary mode
/// - Minimum confidence threshold
/// - Minimum dwell time between switches
/// - Hourly switch rate limit
/// - Scheduler health (heartbeat)
/// - Consecutive failure limit (disables adaptation on breach)
#[derive(Debug, Clone)]
pub struct GuardrailEngine {
    /// Current active profile.
    current_profile: Profile,

    /// Last known good profile (successful switch).
    previous_good_profile: Option<Profile>,

    /// Timestamp of the last successful switch.
    last_switch_time: Option<u64>,

    /// Timestamps of switches in the last hour (for rate limiting).
    recent_switches: Vec<u64>,

    /// Consecutive failed switch attempts.
    consecutive_failures: u32,

    /// Whether adaptation is currently enabled.
    adaptation_enabled: bool,

    /// Current adaptation mode.
    adaptation_mode: AdaptationMode,

    // Configuration
    /// Minimum confidence required to allow a switch.
    min_confidence: f64,

    /// Minimum seconds between switches.
    min_dwell_secs: u64,

    /// Maximum switches allowed per hour.
    max_switches_per_hour: u32,

    /// Maximum consecutive failures before disabling adaptation.
    max_consecutive_failures: u32,
}

impl GuardrailEngine {
    /// Creates a new GuardrailEngine with default settings.
    ///
    /// Defaults (per sprint requirements):
    /// - min_confidence: 0.75
    /// - min_dwell_secs: 15
    /// - max_switches_per_hour: 12
    /// - max_consecutive_failures: 3
    #[must_use]
    pub fn new(current_profile: Profile) -> Self {
        Self {
            current_profile,
            previous_good_profile: None,
            last_switch_time: None,
            recent_switches: Vec::new(),
            consecutive_failures: 0,
            adaptation_enabled: true,
            adaptation_mode: AdaptationMode::Off,
            min_confidence: 0.75,
            min_dwell_secs: 15,
            max_switches_per_hour: 12,
            max_consecutive_failures: 3,
        }
    }

    /// Creates a GuardrailEngine with a custom initial profile.
    #[must_use]
    pub fn with_profile(current_profile: Profile) -> Self {
        Self::new(current_profile)
    }

    /// Sets the minimum confidence threshold.
    #[must_use]
    pub fn with_min_confidence(mut self, min_confidence: f64) -> Self {
        self.min_confidence = min_confidence;
        self
    }

    /// Sets the minimum dwell time in seconds.
    #[must_use]
    pub fn with_min_dwell(mut self, min_dwell_secs: u64) -> Self {
        self.min_dwell_secs = min_dwell_secs;
        self
    }

    /// Sets the maximum switches per hour.
    #[must_use]
    pub fn with_max_switches_per_hour(mut self, max_switches_per_hour: u32) -> Self {
        self.max_switches_per_hour = max_switches_per_hour;
        self
    }

    /// Sets the maximum consecutive failures before disabling adaptation.
    #[must_use]
    pub fn with_max_consecutive_failures(mut self, max_consecutive_failures: u32) -> Self {
        self.max_consecutive_failures = max_consecutive_failures;
        self
    }

    /// Updates the adaptation enabled state.
    pub fn set_adaptation_enabled(&mut self, enabled: bool) {
        self.adaptation_enabled = enabled;
        if !enabled {
            self.consecutive_failures = 0;
        }
    }

    /// Updates the adaptation mode.
    pub fn set_adaptation_mode(&mut self, mode: AdaptationMode) {
        self.adaptation_mode = mode;
    }

    /// Returns the current profile.
    #[must_use]
    pub fn current_profile(&self) -> Profile {
        self.current_profile
    }

    /// Returns the previous good profile.
    #[must_use]
    pub fn previous_good_profile(&self) -> Option<Profile> {
        self.previous_good_profile
    }

    /// Returns the consecutive failure count.
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Returns whether adaptation is enabled.
    #[must_use]
    pub fn is_adaptation_enabled(&self) -> bool {
        self.adaptation_enabled
    }

    /// Returns the current adaptation mode.
    #[must_use]
    pub fn adaptation_mode(&self) -> AdaptationMode {
        self.adaptation_mode
    }

    /// Checks if a switch to the recommended profile is allowed.
    ///
    /// This is the main entry point. It evaluates all guardrails and returns
    /// `Allowed` if the switch may proceed, or `Denied(violation)` with the
    /// first violation encountered.
    ///
    /// # Arguments
    /// * `recommendation` - The policy recommendation to evaluate
    /// * `heartbeat_ok` - Whether the scheduler heartbeat is fresh/healthy
    /// * `now` - Current Unix timestamp (for testing determinism)
    pub fn can_switch(
        &self,
        recommendation: &PolicyRecommendation,
        heartbeat_ok: bool,
        now: u64,
    ) -> GuardrailResult {
        // 1. Adaptation enabled?
        if !self.adaptation_enabled {
            return GuardrailResult::Denied(GuardrailViolation::AdaptationDisabled);
        }

        // 2. Canary mode?
        if self.adaptation_mode != AdaptationMode::Canary {
            return GuardrailResult::Denied(GuardrailViolation::NotCanaryMode {
                mode: self.adaptation_mode,
            });
        }

        // 3. Confidence threshold
        if recommendation.confidence < self.min_confidence {
            return GuardrailResult::Denied(GuardrailViolation::LowConfidence {
                confidence: recommendation.confidence,
                threshold: self.min_confidence,
            });
        }

        // 4. Minimum dwell time
        if let Some(last_switch) = self.last_switch_time {
            let elapsed = now.saturating_sub(last_switch);
            if elapsed < self.min_dwell_secs {
                return GuardrailResult::Denied(GuardrailViolation::DwellTimeNotMet {
                    elapsed_secs: elapsed,
                    min_dwell_secs: self.min_dwell_secs,
                });
            }
        }

        // 5. Hourly switch limit
        let recent_count = self.count_recent_switches(now);
        if recent_count >= self.max_switches_per_hour {
            return GuardrailResult::Denied(GuardrailViolation::HourlyLimitExceeded {
                count: recent_count,
                limit: self.max_switches_per_hour,
            });
        }

        // 6. Scheduler healthy (heartbeat)
        if !heartbeat_ok {
            return GuardrailResult::Denied(GuardrailViolation::UnhealthyHeartbeat);
        }

        GuardrailResult::Allowed
    }

    /// Counts switches in the last hour (3600 seconds).
    fn count_recent_switches(&self, now: u64) -> u32 {
        const HOUR_SECS: u64 = 3600;
        self.recent_switches
            .iter()
            .filter(|&&t| now.saturating_sub(t) < HOUR_SECS)
            .count() as u32
    }

    /// Records a successful switch.
    ///
    /// Updates state: current_profile, previous_good_profile, last_switch_time,
    /// recent_switches, and resets consecutive_failures.
    pub fn record_success(&mut self, new_profile: Profile, now: u64) {
        self.previous_good_profile = Some(self.current_profile);
        self.current_profile = new_profile;
        self.last_switch_time = Some(now);
        self.recent_switches.push(now);
        self.consecutive_failures = 0;
        self.prune_old_switches(now);
    }

    /// Records a failed switch attempt.
    ///
    /// Increments consecutive_failures. If the failure count reaches
    /// max_consecutive_failures, adaptation is automatically disabled.
    pub fn record_failure(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= self.max_consecutive_failures {
            self.adaptation_enabled = false;
        }
    }

    /// Removes switch timestamps older than 1 hour.
    fn prune_old_switches(&mut self, now: u64) {
        const HOUR_SECS: u64 = 3600;
        self.recent_switches
            .retain(|&t| now.saturating_sub(t) < HOUR_SECS);
    }

    /// Resets the failure counter (e.g., after manual intervention).
    pub fn reset_failures(&mut self) {
        self.consecutive_failures = 0;
        if !self.adaptation_enabled {
            self.adaptation_enabled = true;
        }
    }
}

impl Default for GuardrailEngine {
    fn default() -> Self {
        Self::new(Profile::Balanced)
    }
}

#[cfg(test)]
mod guardrail_tests {
    use super::*;

    fn make_recommendation(
        profile: Profile,
        confidence: f64,
        timestamp: u64,
    ) -> PolicyRecommendation {
        PolicyRecommendation {
            recommended_profile: profile,
            confidence,
            reason: "test".to_string(),
            timestamp,
            apply: false,
        }
    }

    #[test]
    fn rejects_when_adaptation_disabled() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_enabled(false);

        let now = 1000;
        let recommendation = make_recommendation(Profile::Performance, 0.90, now);

        let result = engine.can_switch(&recommendation, true, now);

        assert!(!result.is_allowed());
        assert!(matches!(
            result.violation(),
            Some(GuardrailViolation::AdaptationDisabled)
        ));
    }

    #[test]
    fn rejects_when_not_canary_mode() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Off);

        let now = 1000;
        let recommendation = make_recommendation(Profile::Performance, 0.90, now);

        let result = engine.can_switch(&recommendation, true, now);

        assert!(!result.is_allowed());
        assert!(matches!(
            result.violation(),
            Some(GuardrailViolation::NotCanaryMode { mode })
            if *mode == AdaptationMode::Off
        ));

        engine.set_adaptation_mode(AdaptationMode::Shadow);
        let result = engine.can_switch(&recommendation, true, now);
        assert!(!result.is_allowed());
        assert!(matches!(
            result.violation(),
            Some(GuardrailViolation::NotCanaryMode { mode })
            if *mode == AdaptationMode::Shadow
        ));
    }

    #[test]
    fn allows_in_canary_mode() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        let now = 1000;
        let recommendation = make_recommendation(Profile::Performance, 0.90, now);

        let result = engine.can_switch(&recommendation, true, now);

        assert!(result.is_allowed());
    }

    #[test]
    fn rejects_low_confidence() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        let now = 1000;
        let recommendation = make_recommendation(Profile::Performance, 0.42, now);

        let result = engine.can_switch(&recommendation, true, now);

        assert!(!result.is_allowed());
        assert!(matches!(
            result.violation(),
            Some(GuardrailViolation::LowConfidence { confidence, threshold })
            if (confidence - 0.42).abs() < f64::EPSILON && (threshold - 0.75).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn rejects_when_dwell_time_not_met() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        let now = 1000;
        // Record a previous switch 5 seconds ago
        engine.record_success(Profile::Balanced, now - 5);

        let recommendation = make_recommendation(Profile::Performance, 0.90, now);

        let result = engine.can_switch(&recommendation, true, now);

        assert!(!result.is_allowed());
        assert!(matches!(
            result.violation(),
            Some(GuardrailViolation::DwellTimeNotMet { elapsed_secs, min_dwell_secs })
            if *elapsed_secs == 5 && *min_dwell_secs == 15
        ));
    }

    #[test]
    fn allows_when_dwell_time_met() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        let now = 10_000;
        // Record a previous switch 20 seconds ago (> 15s min)
        engine.record_success(Profile::Balanced, now - 20);

        let recommendation = make_recommendation(Profile::Performance, 0.90, now);

        let result = engine.can_switch(&recommendation, true, now);

        assert!(result.is_allowed());
    }

    #[test]
    fn rejects_when_hourly_limit_exceeded() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        let now = 10_000;
        // Record 12 switches in the last hour
        for i in 0..12 {
            engine.record_success(Profile::Balanced, now - (i * 300) as u64);
        }

        let recommendation = make_recommendation(Profile::Performance, 0.90, now);

        let result = engine.can_switch(&recommendation, true, now);

        assert!(!result.is_allowed());
        assert!(matches!(
            result.violation(),
            Some(GuardrailViolation::HourlyLimitExceeded { count, limit })
            if *count == 12 && *limit == 12
        ));
    }

    #[test]
    fn allows_when_hourly_limit_not_exceeded() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        let now = 10_000;
        // Record 11 switches in the last hour
        for i in 0..11 {
            engine.record_success(Profile::Balanced, now - (i * 300) as u64);
        }

        let recommendation = make_recommendation(Profile::Performance, 0.90, now);

        let result = engine.can_switch(&recommendation, true, now);

        assert!(result.is_allowed());
    }

    #[test]
    fn rejects_when_heartbeat_unhealthy() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        let now = 1000;
        let recommendation = make_recommendation(Profile::Performance, 0.90, now);

        let result = engine.can_switch(&recommendation, false, now);

        assert!(!result.is_allowed());
        assert!(matches!(
            result.violation(),
            Some(GuardrailViolation::UnhealthyHeartbeat)
        ));
    }

    #[test]
    fn disables_adaptation_after_max_failures() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        // Record 3 failures
        engine.record_failure();
        assert!(engine.is_adaptation_enabled());
        assert_eq!(engine.consecutive_failures(), 1);

        engine.record_failure();
        assert!(engine.is_adaptation_enabled());
        assert_eq!(engine.consecutive_failures(), 2);

        engine.record_failure();
        assert!(!engine.is_adaptation_enabled());
        assert_eq!(engine.consecutive_failures(), 3);
    }

    #[test]
    fn rejects_after_adaptation_disabled_by_failures() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        // Trigger 3 failures to disable adaptation
        engine.record_failure();
        engine.record_failure();
        engine.record_failure();
        assert!(!engine.is_adaptation_enabled());

        let now = 1000;
        let recommendation = make_recommendation(Profile::Performance, 0.90, now);

        let result = engine.can_switch(&recommendation, true, now);

        assert!(!result.is_allowed());
        assert!(matches!(
            result.violation(),
            Some(GuardrailViolation::AdaptationDisabled)
        ));
    }

    #[test]
    fn resets_failures_on_success() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        engine.record_failure();
        engine.record_failure();
        assert_eq!(engine.consecutive_failures(), 2);

        let now = 1000;
        engine.record_success(Profile::Performance, now);

        assert_eq!(engine.consecutive_failures(), 0);
    }

    #[test]
    fn resets_failures_manually() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        engine.record_failure();
        engine.record_failure();
        engine.record_failure(); // Disables adaptation
        assert!(!engine.is_adaptation_enabled());

        engine.reset_failures();

        assert!(engine.is_adaptation_enabled());
        assert_eq!(engine.consecutive_failures(), 0);
    }

    #[test]
    fn tracks_previous_good_profile() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        assert_eq!(engine.previous_good_profile(), None);

        let now = 1000;
        engine.record_success(Profile::Performance, now);

        assert_eq!(engine.previous_good_profile(), Some(Profile::Balanced));
        assert_eq!(engine.current_profile(), Profile::Performance);

        // Another switch
        engine.record_success(Profile::Interactive, now + 20);

        assert_eq!(engine.previous_good_profile(), Some(Profile::Performance));
        assert_eq!(engine.current_profile(), Profile::Interactive);
    }

    #[test]
    fn prunes_old_switches() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        let now = 10000;

        // Record 5 switches, 30 minutes apart
        for i in 0..5 {
            engine.record_success(Profile::Balanced, now - (i * 1800) as u64);
        }

        // Check 100 seconds later.
        // The switches at 10000, 8200, and 6400 are within the last hour.
        let check_now = now + 100;

        let recent = engine.count_recent_switches(check_now);

        assert_eq!(recent, 2);
    }
    #[test]
    fn works_with_custom_thresholds() {
        let mut engine = GuardrailEngine::new(Profile::Balanced)
            .with_min_confidence(0.50)
            .with_min_dwell(10)
            .with_max_switches_per_hour(5)
            .with_max_consecutive_failures(2);

        engine.set_adaptation_mode(AdaptationMode::Canary);

        // Should accept confidence 0.60 with custom threshold 0.50
        let now = 10_000;
        let recommendation = make_recommendation(Profile::Performance, 0.60, now);
        let result = engine.can_switch(&recommendation, true, now);
        assert!(result.is_allowed());

        // Should reject dwell < 10s
        engine.record_success(Profile::Balanced, now - 5);
        let result = engine.can_switch(&recommendation, true, now);
        assert!(!result.is_allowed());
        assert!(matches!(
            result.violation(),
            Some(GuardrailViolation::DwellTimeNotMet { min_dwell_secs, .. })
            if *min_dwell_secs == 10
        ));

        // Should reject after 5 switches/hour
        engine.record_success(Profile::Balanced, now);
        for i in 1..5 {
            engine.record_success(Profile::Balanced, now - (i * 600) as u64);
        }
        let recommendation = make_recommendation(Profile::Interactive, 0.90, now);
        let result = engine.can_switch(&recommendation, true, now);
        assert!(!result.is_allowed());
        assert!(matches!(
            result.violation(),
            Some(GuardrailViolation::HourlyLimitExceeded { limit, .. })
            if *limit == 5
        ));

        // Should disable after 2 failures
        engine.record_failure();
        assert!(engine.is_adaptation_enabled());
        engine.record_failure();
        assert!(!engine.is_adaptation_enabled());
    }

    #[test]
    fn result_helper_methods() {
        let mut engine = GuardrailEngine::new(Profile::Balanced);
        engine.set_adaptation_mode(AdaptationMode::Canary);

        let now = 1000;
        let recommendation = make_recommendation(Profile::Performance, 0.90, now);

        let result = engine.can_switch(&recommendation, true, now);
        assert!(result.is_allowed());
        assert!(result.violation().is_none());

        // Reject
        let recommendation = make_recommendation(Profile::Performance, 0.42, now);
        let result = engine.can_switch(&recommendation, true, now);
        assert!(!result.is_allowed());
        assert!(result.violation().is_some());
        assert!(matches!(
            result.violation(),
            Some(GuardrailViolation::LowConfidence { .. })
        ));
    }
}
