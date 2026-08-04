use serde::{Deserialize, Serialize};

use crate::profile::Profile;

/// Internal request type used by the daemon.
///
/// These requests are independent of the wire protocol and represent the
/// operations that the daemon can perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Status,
    GetProfile,
    SetProfile(Profile),
    PauseAdaptation,
    ResumeAdaptation,
}

/// Request messages sent over the IPC socket.
///
/// These are serialized to JSON and transported between the CLI and daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SocketRequest {
    Status,

    GetProfile,

    SetProfile { profile: Profile },

    PauseAdaptation,

    ResumeAdaptation,

    Shutdown,

    Ping,
}

impl From<SocketRequest> for Request {
    fn from(request: SocketRequest) -> Self {
        match request {
            SocketRequest::Status => Request::Status,

            SocketRequest::GetProfile => Request::GetProfile,

            SocketRequest::SetProfile { profile } => Request::SetProfile(profile),

            SocketRequest::PauseAdaptation => Request::PauseAdaptation,

            SocketRequest::ResumeAdaptation => Request::ResumeAdaptation,

            SocketRequest::Shutdown | SocketRequest::Ping => Request::Status,
        }
    }
}

impl SocketRequest {
    pub fn command_name(&self) -> &'static str {
        match self {
            SocketRequest::Status => "status",
            SocketRequest::GetProfile => "get-profile",
            SocketRequest::SetProfile { .. } => "set-profile",
            SocketRequest::PauseAdaptation => "pause-adaptation",
            SocketRequest::ResumeAdaptation => "resume-adaptation",
            SocketRequest::Shutdown => "shutdown",
            SocketRequest::Ping => "ping",
        }
    }
}
