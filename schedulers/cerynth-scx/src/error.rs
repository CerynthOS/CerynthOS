use std::fmt;

#[derive(Debug)]
pub enum SchedError {
    Dequeue(i32),
    Dispatch(libbpf_rs::Error),
    /// A dispatch failure simulated by `CERYNTH_SCX_INJECT_DISPATCH_ERRORS`,
    /// used by `scripts/scx/fault-test.sh` to exercise the retry path.
    InjectedDispatch,
}

impl fmt::Display for SchedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchedError::Dequeue(errno) => write!(f, "dequeue_task failed (errno {errno})"),
            SchedError::Dispatch(e) => write!(f, "dispatch_task failed: {e}"),
            SchedError::InjectedDispatch => write!(f, "dispatch_task failed (injected fault)"),
        }
    }
}
