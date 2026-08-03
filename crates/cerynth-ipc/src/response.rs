use serde::{Deserialize, Serialize};

use crate::backend::SchedulerStatus;
use crate::profile::Profile;

/// Response messages sent from the daemon to the CLI over the IPC socket.
///
/// Each variant represents a possible response to a client request.
/// All variants are serializable to JSON for transport over Unix domain sockets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SocketResponse {
    /// Successful response containing the current scheduler status.
    Status {
        /// The current scheduler status.
        status: SchedulerStatus,
    },

    /// Successful response containing the current profile.
    Profile {
        /// The currently active profile.
        profile: Profile,
    },

    /// Generic success response for operations that don't return data.
    Success,

    /// Error response with a descriptive message.
    Error {
        /// Human-readable error description.
        message: String,
    },

    /// Response to a ping request.
    Pong,
}

impl SocketResponse {
    /// Returns true if this response indicates success (not an error).
    pub fn is_success(&self) -> bool {
        !matches!(self, SocketResponse::Error { .. })
    }

    /// Returns the error message if this is an error response.
    pub fn error_message(&self) -> Option<&str> {
        match self {
            SocketResponse::Error { message } => Some(message),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_response_status_serialization() {
        let status = SchedulerStatus {
            profile: Profile::Interactive,
            adaptation_enabled: true,
            backend: crate::backend::SchedulerBackend::Mock,
        };
        let resp = SocketResponse::Status { status };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: SocketResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
        assert!(parsed.is_success());
    }

    #[test]
    fn socket_response_profile_serialization() {
        let resp = SocketResponse::Profile {
            profile: Profile::Performance,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"type":"profile","profile":"performance"}"#);

        let parsed: SocketResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
        assert!(parsed.is_success());
    }

    #[test]
    fn socket_response_success_serialization() {
        let resp = SocketResponse::Success;
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"type":"success"}"#);

        let parsed: SocketResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
        assert!(parsed.is_success());
    }

    #[test]
    fn socket_response_error_serialization() {
        let resp = SocketResponse::Error {
            message: "invalid profile".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"type":"error","message":"invalid profile"}"#);

        let parsed: SocketResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
        assert!(!parsed.is_success());
        assert_eq!(parsed.error_message(), Some("invalid profile"));
    }

    #[test]
    fn socket_response_pong_serialization() {
        let resp = SocketResponse::Pong;
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"type":"pong"}"#);

        let parsed: SocketResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
        assert!(parsed.is_success());
    }
}