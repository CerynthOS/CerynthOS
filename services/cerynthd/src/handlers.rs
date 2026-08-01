use crate::backend::Backend;
use cerynth_ipc::{Request, Response};

pub fn handle_request<B: Backend>(
    backend: &mut B,
    request: Request,
) -> Response {
    match request {
        Request::Status => {
            Response::Status(backend.status())
        }

        Request::GetProfile => {
            Response::Profile(backend.get_profile())
        }

        Request::SetProfile(profile) => {
            backend.set_profile(profile);
            Response::Success
        }

        Request::PauseAdaptation => {
            backend.pause_adaptation();
            Response::Success
        }

        Request::ResumeAdaptation => {
            backend.resume_adaptation();
            Response::Success
        }
    }
}
