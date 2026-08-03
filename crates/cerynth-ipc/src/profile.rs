use serde::{Deserialize, Serialize};

/// Represents a scheduler performance profile.
///
/// Each profile tunes the scheduler for a specific workload pattern:
/// - `Balanced`: Default profile for general-purpose workloads
/// - `Interactive`: Optimized for low-latency interactive applications
/// - `Performance`: Maximizes throughput for compute-intensive workloads
/// - `Background`: Minimizes resource usage for background/batch workloads
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Balanced,
    Interactive,
    Performance,
    Background,
}

impl Profile {
    /// Returns all available profiles in declaration order.
    pub fn all() -> [Profile; 4] {
        [
            Profile::Balanced,
            Profile::Interactive,
            Profile::Performance,
            Profile::Background,
        ]
    }
}

impl std::fmt::Display for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Profile::Balanced => write!(f, "balanced"),
            Profile::Interactive => write!(f, "interactive"),
            Profile::Performance => write!(f, "performance"),
            Profile::Background => write!(f, "background"),
        }
    }
}

impl std::str::FromStr for Profile {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "balanced" => Ok(Profile::Balanced),
            "interactive" => Ok(Profile::Interactive),
            "performance" => Ok(Profile::Performance),
            "background" => Ok(Profile::Background),
            _ => Err(format!("unknown profile: {}", s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_serialization() {
        let profile = Profile::Balanced;
        let json = serde_json::to_string(&profile).unwrap();
        assert_eq!(json, "\"balanced\"");

        let profile: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(profile, Profile::Balanced);
    }

    #[test]
    fn profile_all_variants() {
        for profile in Profile::all() {
            let json = serde_json::to_string(&profile).unwrap();
            let parsed: Profile = serde_json::from_str(&json).unwrap();
            assert_eq!(profile, parsed);
        }
    }

    #[test]
    fn profile_from_str() {
        assert_eq!("balanced".parse::<Profile>().unwrap(), Profile::Balanced);
        assert_eq!("interactive".parse::<Profile>().unwrap(), Profile::Interactive);
        assert_eq!("performance".parse::<Profile>().unwrap(), Profile::Performance);
        assert_eq!("background".parse::<Profile>().unwrap(), Profile::Background);
        assert!("invalid".parse::<Profile>().is_err());
    }

    #[test]
    fn profile_display() {
        assert_eq!(Profile::Balanced.to_string(), "balanced");
        assert_eq!(Profile::Interactive.to_string(), "interactive");
        assert_eq!(Profile::Performance.to_string(), "performance");
        assert_eq!(Profile::Background.to_string(), "background");
    }
}