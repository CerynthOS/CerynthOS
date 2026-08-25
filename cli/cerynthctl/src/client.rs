use cerynth_ipc::{Request, RequestEnvelope, ResponseEnvelope, SocketRequest};

use crate::transport::send_request;

pub fn execute(request: Request) -> std::io::Result<ResponseEnvelope> {
    let socket_request = match request {
        Request::Status => SocketRequest::Status,

        Request::GetProfile => SocketRequest::GetProfile,

        Request::GetAdaptationMode => SocketRequest::GetAdaptationMode,

        Request::SetAdaptationMode(mode) => SocketRequest::SetAdaptationMode { mode },

        Request::PauseAdaptation => SocketRequest::PauseAdaptation,

        Request::ResumeAdaptation => SocketRequest::ResumeAdaptation,

        Request::SetProfile(profile) => SocketRequest::SetProfile { profile },
        Request::Start => SocketRequest::Start,

        Request::Stop => SocketRequest::Stop,

        Request::Restart => SocketRequest::Restart,
    };

    let envelope = RequestEnvelope::new(socket_request);

    send_request(envelope)
}
