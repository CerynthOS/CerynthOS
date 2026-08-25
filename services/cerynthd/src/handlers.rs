use crate::backend::Backend;
use cerynth_ipc::{AdaptationMode, Request, Response};

pub fn handle_request<B: Backend + ?Sized>(backend: &mut B, request: Request) -> Response {
    match request {
        Request::Status => match backend.status() {
            Ok(status) => Response::Status(status),
            Err(e) => Response::Error(e),
        },

        Request::GetProfile => match backend.get_profile() {
            Ok(profile) => Response::Profile(profile),
            Err(e) => Response::Error(e),
        },

        Request::SetProfile(profile) => match backend.set_profile(profile) {
            Ok(()) => Response::Success,
            Err(e) => Response::Error(e),
        },

        Request::GetAdaptationMode => match backend.get_adaptation_mode() {
            Ok(mode) => Response::AdaptationMode(mode),
            Err(e) => Response::Error(e),
        },

        Request::SetAdaptationMode(mode) => match backend.set_adaptation_mode(mode) {
            Ok(()) => Response::Success,
            Err(e) => Response::Error(e),
        },

        Request::PauseAdaptation => match backend.pause_adaptation() {
            Ok(()) => Response::Success,
            Err(e) => Response::Error(e),
        },

        Request::ResumeAdaptation => match backend.resume_adaptation() {
            Ok(()) => Response::Success,
            Err(e) => Response::Error(e),
        },

        Request::Start => match backend.start() {
            Ok(()) => Response::Success,
            Err(e) => Response::Error(e),
        },

        Request::Stop => match backend.stop() {
            Ok(()) => Response::Success,
            Err(e) => Response::Error(e),
        },

        Request::Restart => match backend.restart() {
            Ok(()) => Response::Success,
            Err(e) => Response::Error(e),
        },
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{backends::mock::MockBackend, state::DaemonState};

    use cerynth_ipc::{Profile, SchedulerBackend};

    fn backend() -> MockBackend {
        MockBackend::new(DaemonState {
            profile: Profile::Balanced,
            adaptation_enabled: false,
            adaptation_mode: AdaptationMode::Off,
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

        assert_eq!(backend.get_profile().unwrap(), Profile::Performance);
    }

    #[test]
    fn pause_resume_request() {
        let mut backend = backend();

        handle_request(&mut backend, Request::PauseAdaptation);

        assert!(!backend.status().unwrap().adaptation_enabled);

        handle_request(&mut backend, Request::ResumeAdaptation);

        assert!(backend.status().unwrap().adaptation_enabled);
    }

    #[test]
    fn lifecycle_requests() {
        let mut backend = backend();

        assert_eq!(
            handle_request(&mut backend, Request::Start),
            Response::Success
        );
        assert_eq!(
            handle_request(&mut backend, Request::Restart),
            Response::Success
        );
        assert_eq!(
            handle_request(&mut backend, Request::Stop),
            Response::Success
        );
    }
}
