use serde::{Deserialize, Serialize};

use crate::profile::Profile;

/// Request messages sent from the CLI to the daemon over the IPC socket.
///
/// Each variant represents a distinct operation the client can request.
/// All variants are serializable to JSON for transport over Unix domain sockets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SocketRequest {
    /// Request the current scheduler status (profile, adaptation state, backend).
    Status,

    /// Request the currently active profile.
    GetProfile,

    /// Set the active scheduler profile.
    SetProfile {
        /// The profile to activate.
        profile: Profile,
    },

    /// Pause automatic profile adaptation.
    PauseAdaptation,

    /// Resume automatic profile adaptation.
    ResumeAdaptation,

    /// Request daemon shutdown (graceful).
    Shutdown,

    /// Ping the daemon to check liveness.
    Ping,
}

impl SocketRequest {
    /// Returns the command name associated with this request variant.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_request_status_serialization() {
        let req = SocketRequest::Status;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"status"}"#);

        let parsed: SocketRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, parsed);
    }

    #[test]
    fn socket_request_get_profile_serialization() {
        let req = SocketRequest::GetProfile;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"get-profile"}"#);

        let parsed: SocketRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, parsed);
    }

    #[test]
    fn socket_request_set_profile_serialization() {
        let req = SocketRequest::SetProfile {
            profile: Profile::Performance,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"set-profile","profile":"performance"}"#);

        let parsed: SocketRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, parsed);
    }

    #[test]
    fn socket_request_pause_adaptation_serialization() {
        let req = SocketRequest::PauseAdaptation;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"pause-adaptation"}"#);

        let parsed: SocketRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, parsed);
    }

    #[test]
    fn socket_request_resume_adaptation_serialization() {
        let req = SocketRequest::ResumeAdaptation;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"resume-adaptation"}"#);

        let parsed: SocketRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, parsed);
    }

    #[test]
    fn socket_request_shutdown_serialization() {
        let req = SocketRequest::Shutdown;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"shutdown"}"#);

        let parsed: SocketRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, parsed);
    }

    #[test]
    fn socket_request_ping_serialization() {
        let req = SocketRequest::Ping;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"ping"}"#);

        let parsed: SocketRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, parsed);
    }

    #[test]
    fn socket_request_command_names() {
        assert_eq!(SocketRequest::Status.command_name(), "status");
        assert_eq!(SocketRequest::GetProfile.command_name(), "get-profile");
        assert_eq!(SocketRequest::SetProfile { profile: Profile::Balanced }.command_name(), "set-profile");
        assert_eq!(SocketRequest::PauseAdaptation.command_name(), "pause-adaptation");
        assert_eq!(SocketRequest::ResumeAdaptation.command_name(), "resume-adaptation");
        assert_eq!(SocketRequest::Shutdown.command_name(), "shutdown");
        assert_eq!(SocketRequest::Ping.command_name(), "ping");
    }
}