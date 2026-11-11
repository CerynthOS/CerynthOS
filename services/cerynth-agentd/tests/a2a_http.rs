use anyhow::Result;

#[test]
fn independent_http_client_checks_sponsor_scope_and_protocol_limits() -> Result<()> {
    let report = tempfile::tempdir()?;
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new("python3")
        .arg(repository.join("tools/a2a_runtime_smoke.py"))
        .args(["--binary", env!("CARGO_BIN_EXE_cerynth-agentd"), "--output"])
        .arg(report.path().join("report.json"))
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let evidence: serde_json::Value =
        serde_json::from_slice(&std::fs::read(report.path().join("report.json"))?)?;
    assert_eq!(evidence["sponsor_substitution_denied"], true);
    assert_eq!(evidence["automatic_execution"], false);
    Ok(())
}
