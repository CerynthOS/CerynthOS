use cerynth_ipc::Profile;

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
