use cerynth_ipc::{
    Request,
    RequestEnvelope,
    ResponseEnvelope,
    SocketRequest,
};

use crate::transport::send_request;

pub fn execute(
    request: Request,
) -> std::io::Result<ResponseEnvelope> {
    let socket_request = match request {
        Request::Status => SocketRequest::Status,

        Request::GetProfile => SocketRequest::GetProfile,

        Request::PauseAdaptation => SocketRequest::PauseAdaptation,

        Request::ResumeAdaptation => SocketRequest::ResumeAdaptation,

        Request::SetProfile(profile) => {
            SocketRequest::SetProfile { profile }
        }
    };

    let envelope = RequestEnvelope::new(socket_request);

    send_request(envelope)
}
