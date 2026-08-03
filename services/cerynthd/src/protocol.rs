use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SocketRequest {
    pub command: String,
    pub profile: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SocketResponse {
    pub status: String,
    pub scheduler: Option<String>,
    pub profile: Option<String>,
    pub adaptation_enabled: Option<bool>,
    pub message: Option<String>,
}
