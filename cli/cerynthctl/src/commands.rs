use cerynth_ipc::{Profile, Request};

pub fn parse_command(args: &[String]) -> Option<Request> {
    if args.len() < 2 {
        return None;
    }

    match args[1].as_str() {
        "status" => Some(Request::Status),

        "profile" => {
            if args.len() < 3 {
                return None;
            }

            match args[2].as_str() {
                "get" => Some(Request::GetProfile),

                "set" => {
                    if args.len() < 4 {
                        return None;
                    }

                    let profile = match args[3].as_str() {
                        "balanced" => Profile::Balanced,
                        "interactive" => Profile::Interactive,
                        "performance" => Profile::Performance,
                        "background" => Profile::Background,
                        _ => return None,
                    };

                    Some(Request::SetProfile(profile))
                }

                _ => None,
            }
        }

        "adaptation" => {
            if args.len() < 3 {
                return None;
            }

            match args[2].as_str() {
                "pause" => Some(Request::PauseAdaptation),
                "resume" => Some(Request::ResumeAdaptation),
                _ => None,
            }
        }

        _ => None,
    }
}
