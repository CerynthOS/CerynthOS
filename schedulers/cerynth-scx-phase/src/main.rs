// The experimental target imports a checked, unchanged baseline at build time.
// Only the bounded, preloaded phase slice policy is substituted.
include!(concat!(env!("OUT_DIR"), "/phase_experiment.rs"));
