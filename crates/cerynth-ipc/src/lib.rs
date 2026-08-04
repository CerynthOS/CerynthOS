//! Shared IPC types used by both the CerynthOS daemon and CLI.

pub mod backend;
pub mod profile;
pub mod protocol;
pub mod request;
pub mod response;

// Re-export the public API.
pub use backend::{SchedulerBackend, SchedulerStatus};
pub use profile::Profile;

pub use protocol::{
    Frame,
    RequestEnvelope,
    ResponseEnvelope,
    VersionError,
    DEFAULT_SOCKET_PATH,
    MAX_MESSAGE_SIZE,
    MIN_PROTOCOL_VERSION,
    PROTOCOL_VERSION,
};

pub use request::{Request, SocketRequest};
pub use response::{Response, SocketResponse};
