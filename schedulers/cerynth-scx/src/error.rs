use std::fmt;

#[derive(Debug)]
pub enum SchedError {
    Dequeue(i32),
    Dispatch(libbpf_rs::Error),
}

impl fmt::Display for SchedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self{
            SchedError::Dequeue(errno) => write!(f, "dequeue_task failed (errno {errno})"),
            SchedError::Dispatch(e) => write!(f, "dispatch_task failed: {e}"),
        }
    }
}