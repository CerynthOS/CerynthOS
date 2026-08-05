use serde::{Deserialize, Serialize};

use crate::backend::SchedulerStatus;
use crate::profile::Profile;

/// Internal response type used by the daemon.
///
/// These responses are independent of the wire protocol and represent the
/// logical results returned by the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Status(SchedulerStatus),
    Profile(Profile),
    Success,
    Error(String),
}

/// Response messages sent over the IPC socket.
///
/// These are serialized to JSON before being sent back to the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SocketResponse {
    Status { status: SchedulerStatus },

    Profile { profile: Profile },

    Success,

    Error { message: String },

    Pong,
}

impl From<Response> for SocketResponse {
    fn from(response: Response) -> Self {
        match response {
            Response::Status(status) => SocketResponse::Status { status },

            Response::Profile(profile) => SocketResponse::Profile { profile },

            Response::Success => SocketResponse::Success,

            Response::Error(message) => SocketResponse::Error { message },
        }
    }
}

impl SocketResponse {
    pub fn is_success(&self) -> bool {
        !matches!(self, SocketResponse::Error { .. })
    }

    pub fn error_message(&self) -> Option<&str> {
        match self {
            SocketResponse::Error { message } => Some(message),
            _ => None,
        }
    }
}
