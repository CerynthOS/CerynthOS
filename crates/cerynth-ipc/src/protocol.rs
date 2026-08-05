use serde::{Deserialize, Serialize};

/// Current wire protocol version.
///
/// Increment this when making breaking changes to the message format.
/// The daemon and client should negotiate version on connection.
pub const PROTOCOL_VERSION: u32 = 1;

/// Minimum supported protocol version for backward compatibility.
pub const MIN_PROTOCOL_VERSION: u32 = 1;

/// Maximum message size in bytes (64 KiB).
///
/// Prevents memory exhaustion from malformed or malicious messages.
pub const MAX_MESSAGE_SIZE: usize = 64 * 1024;

/// Default socket path for the daemon.
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/cerynthd.sock";

/// Request envelope for the wire protocol.
///
/// Wraps a [`SocketRequest`] with protocol metadata for versioning
/// and potential future extensions (authentication, correlation IDs, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEnvelope {
    /// Protocol version used for this request.
    pub version: u32,

    /// Optional correlation ID for request/response matching.
    /// Useful for async pipelines or logging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,

    /// The actual request payload.
    pub request: crate::request::SocketRequest,
}

impl RequestEnvelope {
    /// Creates a new request envelope with the current protocol version.
    pub fn new(request: crate::request::SocketRequest) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            correlation_id: None,
            request,
        }
    }

    /// Creates a new request envelope with a correlation ID.
    pub fn with_correlation_id(
        request: crate::request::SocketRequest,
        correlation_id: String,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            correlation_id: Some(correlation_id),
            request,
        }
    }

    /// Validates that the envelope version is supported.
    pub fn validate_version(&self) -> Result<(), VersionError> {
        if self.version < MIN_PROTOCOL_VERSION || self.version > PROTOCOL_VERSION {
            Err(VersionError {
                requested: self.version,
                min_supported: MIN_PROTOCOL_VERSION,
                max_supported: PROTOCOL_VERSION,
            })
        } else {
            Ok(())
        }
    }
}

/// Response envelope for the wire protocol.
///
/// Wraps a [`SocketResponse`] with protocol metadata, echoing the
/// correlation ID from the request for matching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseEnvelope {
    /// Protocol version used for this response.
    pub version: u32,

    /// Correlation ID from the request, if provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,

    /// The actual response payload.
    pub response: crate::response::SocketResponse,
}

impl ResponseEnvelope {
    /// Creates a new response envelope for the given request envelope.
    pub fn for_request(
        request: &RequestEnvelope,
        response: crate::response::SocketResponse,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            correlation_id: request.correlation_id.clone(),
            response,
        }
    }

    /// Creates a new response envelope with an explicit correlation ID.
    pub fn with_correlation_id(
        response: crate::response::SocketResponse,
        correlation_id: String,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            correlation_id: Some(correlation_id),
            response,
        }
    }
}

/// Error indicating an unsupported protocol version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionError {
    /// The version that was requested.
    pub requested: u32,

    /// Minimum supported version.
    pub min_supported: u32,

    /// Maximum supported version.
    pub max_supported: u32,
}

impl std::fmt::Display for VersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unsupported protocol version {} (supported: {}..={})",
            self.requested, self.min_supported, self.max_supported
        )
    }
}

impl std::error::Error for VersionError {}

/// Trait for types that can be framed for transport over the IPC socket.
///
/// Provides methods for serializing to/from JSON with length prefixing
/// for streaming over Unix domain sockets.
pub trait Frame: Serialize + for<'de> Deserialize<'de> {
    /// Serializes this frame to a JSON byte vector.
    fn to_json_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserializes a frame from JSON bytes.
    fn from_json_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Serializes and adds a newline delimiter for line-based framing.
    fn to_line(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut bytes = self.to_json_bytes()?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Parses a frame from a line (newline-delimited JSON).
    fn from_line(line: &[u8]) -> Result<Self, serde_json::Error> {
        // Trim trailing newline
        let trimmed = line
            .strip_suffix(b"\n")
            .or_else(|| line.strip_suffix(b"\r\n"))
            .unwrap_or(line);
        Self::from_json_bytes(trimmed)
    }
}

// Implement Frame for all protocol types
impl Frame for RequestEnvelope {}
impl Frame for ResponseEnvelope {}
impl Frame for crate::request::SocketRequest {}
impl Frame for crate::response::SocketResponse {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::SocketRequest;
    use crate::response::SocketResponse;

    #[test]
    fn protocol_version_constants() {
        assert_eq!(PROTOCOL_VERSION, 1);
        assert_eq!(MIN_PROTOCOL_VERSION, 1);
        assert_eq!(MAX_MESSAGE_SIZE, 64 * 1024);
        assert_eq!(DEFAULT_SOCKET_PATH, "/tmp/cerynthd.sock");
    }

    #[test]
    fn request_envelope_new() {
        let req = SocketRequest::Status;
        let envelope = RequestEnvelope::new(req.clone());

        assert_eq!(envelope.version, PROTOCOL_VERSION);
        assert_eq!(envelope.correlation_id, None);
        assert_eq!(envelope.request, req);
    }

    #[test]
    fn request_envelope_with_correlation_id() {
        let req = SocketRequest::Status;
        let correlation_id = "req-123".to_string();
        let envelope = RequestEnvelope::with_correlation_id(req.clone(), correlation_id.clone());

        assert_eq!(envelope.correlation_id, Some(correlation_id));
    }

    #[test]
    fn request_envelope_version_validation() {
        let mut envelope = RequestEnvelope::new(SocketRequest::Status);
        assert!(envelope.validate_version().is_ok());

        envelope.version = 0;
        assert!(envelope.validate_version().is_err());

        envelope.version = 999;
        assert!(envelope.validate_version().is_err());
    }

    #[test]
    fn response_envelope_for_request() {
        let req =
            RequestEnvelope::with_correlation_id(SocketRequest::Status, "req-123".to_string());
        let resp = SocketResponse::Success;
        let envelope = ResponseEnvelope::for_request(&req, resp);

        assert_eq!(envelope.version, PROTOCOL_VERSION);
        assert_eq!(envelope.correlation_id, Some("req-123".to_string()));
        assert_eq!(envelope.response, SocketResponse::Success);
    }

    #[test]
    fn frame_serialization() {
        let req = SocketRequest::GetProfile;
        let bytes = req.to_json_bytes().unwrap();
        let parsed = SocketRequest::from_json_bytes(&bytes).unwrap();
        assert_eq!(req, parsed);
    }

    #[test]
    fn frame_line_serialization() {
        let req = SocketRequest::Ping;
        let line = req.to_line().unwrap();
        assert!(line.ends_with(b"\n"));

        let parsed = SocketRequest::from_line(&line).unwrap();
        assert_eq!(req, parsed);
    }

    #[test]
    fn request_envelope_serialization() {
        let envelope = RequestEnvelope::new(SocketRequest::SetProfile {
            profile: crate::profile::Profile::Background,
        });

        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: RequestEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(envelope, parsed);
    }

    #[test]
    fn response_envelope_serialization() {
        let envelope = ResponseEnvelope::with_correlation_id(
            SocketResponse::Profile {
                profile: crate::profile::Profile::Balanced,
            },
            "req-456".to_string(),
        );

        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: ResponseEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(envelope, parsed);
    }

    #[test]
    fn deny_unknown_fields() {
        // This should fail because of unknown field
        let json = r#"{"version":1,"request":{"type":"status"},"unknown":true}"#;
        let result: Result<RequestEnvelope, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
