#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Balanced,
    Interactive,
    Performance,
    Background,
}

/// Per-profile tuning knobs. Everything a profile changes about the
/// scheduler lives here or in `policy.rs`, so two profiles can be compared
/// side by side instead of hunting through the dispatch loop.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Starting time slice before any per-task adjustment in
    /// `policy::slice_for`.
    pub base_slice_ns: u64,

    /// Most tasks one dispatch pass handles before handing control back to
    /// the BPF side. Bigger batches mean fewer round trips (less overhead);
    /// smaller batches mean newly woken tasks are looked at sooner.
    pub max_batch: usize,

    /// A task waiting at least this long since it last ran jumps to the
    /// front of its batch. `None` for the FIFO profiles: they never reorder
    /// a batch, so nothing can be pushed back and nothing needs rescuing.
    pub starvation_threshold_ns: Option<u64>,
}

impl Profile {
    /// Name as written in the runtime contract (`docs/contracts/runtime-v1.md`)
    /// and accepted by `--profile`.
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Balanced => "balanced",
            Profile::Interactive => "interactive",
            Profile::Performance => "performance",
            Profile::Background => "background",
        }
    }

    pub fn params(self) -> Params {
        match self {
            Profile::Balanced => Params {
                base_slice_ns: 5_000_000,
                max_batch: 64,
                starvation_threshold_ns: None,
            },
            Profile::Interactive => Params {
                base_slice_ns: 2_000_000,
                max_batch: 32,
                starvation_threshold_ns: Some(20_000_000),
            },
            Profile::Performance => Params {
                base_slice_ns: 10_000_000,
                max_batch: 128,
                starvation_threshold_ns: None,
            },
            Profile::Background => Params {
                base_slice_ns: 20_000_000,
                max_batch: 128,
                starvation_threshold_ns: Some(100_000_000),
            },
        }
    }

    /// Default slice handed to the BPF side at load time. Only used for
    /// tasks the BPF side dispatches on its own without asking us; every
    /// task we dispatch gets an explicit slice from `policy::slice_for`.
    pub fn slice_ns(self) -> u64 {
        self.params().base_slice_ns
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    #[test]
    fn as_str_matches_cli_names() {
        for profile in Profile::value_variants() {
            let cli_name = profile
                .to_possible_value()
                .map(|v| v.get_name().to_string());
            assert_eq!(cli_name.as_deref(), Some(profile.as_str()));
        }
    }

    #[test]
    fn slices_are_ordered_from_interactive_to_background() {
        let slice = |p: Profile| p.params().base_slice_ns;
        assert!(slice(Profile::Interactive) < slice(Profile::Balanced));
        assert!(slice(Profile::Balanced) < slice(Profile::Performance));
        assert!(slice(Profile::Performance) < slice(Profile::Background));
    }
}
