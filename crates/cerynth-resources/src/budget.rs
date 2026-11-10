//! The versioned `ResourceBudget` contract.
//!
//! A [`ResourceBudget`] is a *request*: the limits Person 3 wants a session to
//! have. [`AppliedLimits`] is the *response*: what the kernel actually
//! accepted. The distinction matters — a limit that is merely readable is not
//! a limit that is enforced, and the caller needs to know which one it got.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current schema version of the resource budget contract.
///
/// Bump this for renames, removals, or meaning changes. Adding an optional
/// field does not require a bump.
pub const RESOURCE_BUDGET_SCHEMA_VERSION: u32 = 1;

/// Default CFS period used when a caller does not supply one, in microseconds.
pub const DEFAULT_CPU_PERIOD_US: u64 = 100_000;

/// Shared priority classes used across the platform.
///
/// These are labels, not scheduler changes. The scheduler experiment in later
/// weeks maps them onto policy; this crate only records them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityClass {
    Interactive,
    Balanced,
    Background,
    Inference,
    BatchCompute,
    SystemCritical,
}

impl PriorityClass {
    /// Default `cpu.weight` for this class (cgroup v2 range `1..=10000`).
    #[must_use]
    pub const fn default_cpu_weight(self) -> u64 {
        match self {
            Self::Interactive => 200,
            Self::Balanced => 100,
            Self::Background => 50,
            Self::Inference => 150,
            Self::BatchCompute => 75,
            Self::SystemCritical => 400,
        }
    }
}

/// A CPU bandwidth limit in the `cpu.max` format: `quota period`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuMax {
    /// Quota in microseconds per period. Must be greater than zero.
    pub quota_us: u64,
    /// Period in microseconds. Defaults to [`DEFAULT_CPU_PERIOD_US`].
    pub period_us: u64,
}

impl CpuMax {
    #[must_use]
    pub const fn new(quota_us: u64, period_us: u64) -> Self {
        Self {
            quota_us,
            period_us,
        }
    }

    /// A quota expressed as a fraction of one CPU, using the default period.
    #[must_use]
    pub const fn from_percent(percent: u64) -> Self {
        Self {
            quota_us: DEFAULT_CPU_PERIOD_US * percent / 100,
            period_us: DEFAULT_CPU_PERIOD_US,
        }
    }
}

impl fmt::Display for CpuMax {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.quota_us, self.period_us)
    }
}

impl FromStr for CpuMax {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut parts = value.split_whitespace();
        let quota = parts
            .next()
            .ok_or_else(|| "missing cpu.max quota".to_string())?;
        let period = parts
            .next()
            .ok_or_else(|| "missing cpu.max period".to_string())?;
        if quota == "max" {
            return Err("cpu.max quota is unlimited".to_string());
        }
        Ok(Self {
            quota_us: quota
                .parse()
                .map_err(|_| format!("invalid cpu.max quota: {quota}"))?,
            period_us: period
                .parse()
                .map_err(|_| format!("invalid cpu.max period: {period}"))?,
        })
    }
}

/// Requested limits for one session.
///
/// `None` means "leave this controller alone". It does not mean "unlimited":
/// an absent field is not written, so any value inherited from the parent
/// cgroup stays in force.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_weight: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_max: Option<CpuMax>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_high_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_max_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io_weight: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pids_max: Option<u64>,
}

impl ResourceLimits {
    /// Validates limit values without a session envelope.
    ///
    /// This is shared by [`ResourceBudget::validate`] and the cgroup layer so
    /// an invalid request cannot reach a `cgroup` file even if a caller
    /// forgets to validate the envelope first.
    pub fn validate(&self) -> Result<(), BudgetError> {
        if let Some(weight) = self.cpu_weight {
            if !(1..=10_000).contains(&weight) {
                return Err(BudgetError::CpuWeightOutOfRange(weight));
            }
        }

        if let Some(cpu_max) = self.cpu_max {
            if cpu_max.quota_us == 0 {
                return Err(BudgetError::CpuMaxZeroQuota);
            }
            if !(1_000..=1_000_000).contains(&cpu_max.period_us) {
                return Err(BudgetError::CpuPeriodOutOfRange(cpu_max.period_us));
            }
        }

        if let (Some(high), Some(max)) = (self.memory_high_bytes, self.memory_max_bytes) {
            if high > max {
                return Err(BudgetError::MemoryHighExceedsMax { high, max });
            }
        }

        if let Some(weight) = self.io_weight {
            if !(1..=10_000).contains(&weight) {
                return Err(BudgetError::IoWeightOutOfRange(weight));
            }
        }

        if self.pids_max == Some(0) {
            return Err(BudgetError::PidsMaxZero);
        }

        Ok(())
    }
}

/// A complete, versioned budget request for one agent session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceBudget {
    pub schema_version: u32,
    /// Session UUID from Person 3. Resources are keyed by this, never by PID.
    pub session_id: Uuid,
    pub priority_class: PriorityClass,
    pub limits: ResourceLimits,
}

impl ResourceBudget {
    /// Builds an empty budget at the current schema version.
    #[must_use]
    pub fn new(session_id: Uuid, priority_class: PriorityClass) -> Self {
        Self {
            schema_version: RESOURCE_BUDGET_SCHEMA_VERSION,
            session_id,
            priority_class,
            limits: ResourceLimits::default(),
        }
    }

    /// Validates the request before any cgroup file is written.
    ///
    /// This is the cheap half of admission control: it rejects values the
    /// kernel would reject, or that would be internally inconsistent. Whether
    /// the host can *afford* the budget is Person 3's decision, not this
    /// crate's.
    pub fn validate(&self) -> Result<(), BudgetError> {
        if self.schema_version != RESOURCE_BUDGET_SCHEMA_VERSION {
            return Err(BudgetError::UnsupportedSchemaVersion(self.schema_version));
        }
        self.limits.validate()
    }
}

/// How a limit ended up being applied on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Enforcement {
    /// The file was written and the kernel reports the requested value.
    Enforced,
    /// The value is visible or measurable, but this host could not enforce it.
    ///
    /// Common causes: the controller is delegated read-only, the cgroup is
    /// managed by systemd, or the kernel rounded/rejected the value.
    Observed,
    /// The controller or cgroup file does not exist here at all.
    Unsupported,
}

impl fmt::Display for Enforcement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Enforced => "enforced",
            Self::Observed => "observed",
            Self::Unsupported => "unsupported",
        };
        f.write_str(text)
    }
}

/// The outcome for one requested limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedLimit<T> {
    pub requested: T,
    /// The value read back from the kernel, if it could be read and parsed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied: Option<T>,
    pub enforcement: Enforcement,
    /// Human-readable detail when the outcome was not a clean `Enforced`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Per-limit outcomes for one applied budget.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_weight: Option<AppliedLimit<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_max: Option<AppliedLimit<CpuMax>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_high_bytes: Option<AppliedLimit<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_max_bytes: Option<AppliedLimit<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io_weight: Option<AppliedLimit<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pids_max: Option<AppliedLimit<u64>>,
}

/// The applied state of one session, returned after [`ResourceLimits`] are
/// written to a cgroup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetStatus {
    pub schema_version: u32,
    pub session_id: Uuid,
    pub cgroup_path: String,
    pub limits: AppliedLimits,
}

/// Errors that can be detected before touching a cgroup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetError {
    UnsupportedSchemaVersion(u32),
    CpuWeightOutOfRange(u64),
    CpuMaxZeroQuota,
    CpuPeriodOutOfRange(u64),
    MemoryHighExceedsMax { high: u64, max: u64 },
    IoWeightOutOfRange(u64),
    PidsMaxZero,
}

impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion(version) => {
                write!(f, "unsupported ResourceBudget schema_version {version}")
            }
            Self::CpuWeightOutOfRange(weight) => {
                write!(f, "cpu.weight {weight} is outside 1..=10000")
            }
            Self::CpuMaxZeroQuota => f.write_str("cpu.max quota must be greater than zero"),
            Self::CpuPeriodOutOfRange(period) => {
                write!(f, "cpu.max period {period}us is outside 1000..=1000000")
            }
            Self::MemoryHighExceedsMax { high, max } => {
                write!(f, "memory.high ({high}) exceeds memory.max ({max})")
            }
            Self::IoWeightOutOfRange(weight) => {
                write!(f, "io.weight {weight} is outside 1..=10000")
            }
            Self::PidsMaxZero => f.write_str("pids.max must be at least 1"),
        }
    }
}

impl std::error::Error for BudgetError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> ResourceBudget {
        ResourceBudget::new(Uuid::nil(), PriorityClass::Balanced)
    }

    #[test]
    fn empty_budget_is_valid() {
        assert_eq!(budget().validate(), Ok(()));
    }

    #[test]
    fn cpu_weight_bounds_are_enforced() {
        let mut request = budget();
        request.limits.cpu_weight = Some(0);
        assert_eq!(request.validate(), Err(BudgetError::CpuWeightOutOfRange(0)));
        request.limits.cpu_weight = Some(10_001);
        assert_eq!(
            request.validate(),
            Err(BudgetError::CpuWeightOutOfRange(10_001))
        );
        request.limits.cpu_weight = Some(10_000);
        assert_eq!(request.validate(), Ok(()));
    }

    #[test]
    fn memory_high_cannot_exceed_max() {
        let mut request = budget();
        request.limits.memory_high_bytes = Some(200);
        request.limits.memory_max_bytes = Some(100);
        assert_eq!(
            request.validate(),
            Err(BudgetError::MemoryHighExceedsMax {
                high: 200,
                max: 100
            })
        );
    }

    #[test]
    fn cpu_max_round_trips_through_the_kernel_format() {
        let value = CpuMax::new(50_000, 100_000);
        assert_eq!(value.to_string(), "50000 100000");
        assert_eq!("50000 100000".parse::<CpuMax>(), Ok(value));
        assert!("max 100000".parse::<CpuMax>().is_err());
    }

    #[test]
    fn percent_helper_uses_the_default_period() {
        assert_eq!(
            CpuMax::from_percent(50),
            CpuMax::new(50_000, DEFAULT_CPU_PERIOD_US)
        );
    }

    #[test]
    fn budget_serializes_with_the_contract_field_names() {
        let mut request = budget();
        request.limits.pids_max = Some(64);
        let json = serde_json::to_string(&request).expect("serialize");
        assert!(json.contains("\"schema_version\":1"));
        assert!(json.contains("\"session_id\""));
        assert!(json.contains("\"pids_max\":64"));
        assert!(!json.contains("cpu_weight"), "unset limits are omitted");

        let parsed: ResourceBudget = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, request);
    }

    #[test]
    fn unknown_fields_are_rejected_in_requests() {
        let json = r#"{
            "schema_version": 1,
            "session_id": "00000000-0000-0000-0000-000000000000",
            "priority_class": "balanced",
            "limits": { "cpu_weight": 100 },
            "surprise": true
        }"#;
        assert!(serde_json::from_str::<ResourceBudget>(json).is_err());
    }
}
