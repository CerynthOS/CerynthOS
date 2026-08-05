use cerynth_ipc::{
    Frame,
    RequestEnvelope,
    ResponseEnvelope,
};

pub fn decode_request(
    bytes: &[u8],
) -> Result<RequestEnvelope, serde_json::Error> {
    RequestEnvelope::from_line(bytes)
}

pub fn encode_response(
    response: &ResponseEnvelope,
) -> Result<Vec<u8>, serde_json::Error> {
    response.to_line()
}

