use cerynth_ipc::{AdaptationMode, Profile, Request};

pub fn parse_command(args: &[String]) -> Option<Request> {
    if args.len() < 2 {
        return None;
    }

    match args[1].as_str() {
        "status" => Some(Request::Status),

        "start" => Some(Request::Start),

        "stop" => Some(Request::Stop),

        "restart" => Some(Request::Restart),

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
                "mode" => {
                    if args.len() < 4 {
                        return None;
                    }

                    match args[3].as_str() {
                        "get" => Some(Request::GetAdaptationMode),
                        "set" => {
                            if args.len() < 5 {
                                return None;
                            }

                            let mode = match args[4].as_str() {
                                "off" => AdaptationMode::Off,
                                "shadow" => AdaptationMode::Shadow,
                                "canary" => AdaptationMode::Canary,
                                _ => return None,
                            };

                            Some(Request::SetAdaptationMode(mode))
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        }

        _ => None,
    }
}
