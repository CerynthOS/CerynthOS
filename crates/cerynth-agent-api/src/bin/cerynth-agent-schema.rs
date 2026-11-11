fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schema = match std::env::args().nth(1).as_deref() {
        None => schemars::schema_for!(cerynth_agent_api::Request),
        Some("--session") => schemars::schema_for!(cerynth_agent_api::Session),
        Some(_) => return Err("usage: cerynth-agent-schema [--session]".into()),
    };
    println!("{}", serde_json::to_string_pretty(&schema)?);
    Ok(())
}
