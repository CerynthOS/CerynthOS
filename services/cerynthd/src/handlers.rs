use crate::backend::Backend;
use cerynth_ipc::{Request, Response};

pub fn handle_request<B: Backend>(backend: &mut B, request: Request) -> Response {
    match request {
        Request::Status => Response::Status(backend.status()),

        Request::GetProfile => Response::Profile(backend.get_profile()),

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
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{backend::MockBackend, state::DaemonState};

    use cerynth_ipc::{Profile, SchedulerBackend};

    fn backend() -> MockBackend {
        MockBackend::new(DaemonState {
            profile: Profile::Balanced,
            adaptation_enabled: false,
            scheduler_backend: SchedulerBackend::Mock,
        })
    }

    #[test]
    fn status_request() {
        let mut backend = backend();

        let response = handle_request(&mut backend, Request::Status);

        match response {
            Response::Status(status) => {
                assert_eq!(status.profile, Profile::Balanced);
            }
            _ => panic!("Expected Status response"),
        }
    }

    #[test]
    fn set_profile_request() {
        let mut backend = backend();

        let response = handle_request(&mut backend, Request::SetProfile(Profile::Performance));

        assert_eq!(response, Response::Success);

        assert_eq!(backend.get_profile(), Profile::Performance);
    }

    #[test]
    fn pause_resume_request() {
        let mut backend = backend();

        handle_request(&mut backend, Request::PauseAdaptation);

        assert!(!backend.status().adaptation_enabled);

        handle_request(&mut backend, Request::ResumeAdaptation);

        assert!(backend.status().adaptation_enabled);
    }
}
