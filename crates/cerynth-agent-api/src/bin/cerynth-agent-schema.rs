fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--manifest") {
        println!(
            "{}",
            serde_json::to_string_pretty(&cerynth_agent_api::contract::manifest())?
        );
        return Ok(());
    }
    let schema = match std::env::args().nth(1).as_deref() {
        None => schemars::schema_for!(cerynth_agent_api::Request),
        Some("--session") => schemars::schema_for!(cerynth_agent_api::Session),
        Some(_) => return Err("usage: cerynth-agent-schema [--session|--manifest]".into()),
    };
    println!("{}", serde_json::to_string_pretty(&schema)?);
    Ok(())
}
