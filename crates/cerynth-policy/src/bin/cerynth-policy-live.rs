use std::env;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use cerynth_ipc::Profile;
use cerynth_policy::{Policy, PolicyDecision, PolicyInput, RulesBasedPolicy};
use cerynth_telemetry::{ProcTelemetrySource, TelemetrySource};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct PolicyOutput {
    recommended_profile: Profile,
    confidence: f64,
    reason: String,
    timestamp: u64,
    apply: bool,
}

impl From<PolicyDecision> for PolicyOutput {
    fn from(decision: PolicyDecision) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            recommended_profile: decision.recommended_profile,
            confidence: decision.confidence,
            reason: decision.reason,
            timestamp,
            apply: false,
        }
    }
}

fn output_path() -> PathBuf {
    env::var_os("CERYNTH_POLICY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/cerynth/policy.json"))
}

fn write_policy(output: &PolicyOutput, path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(output).context("serializing policy recommendation")?;

    let tmp = path.with_extension("json.tmp");

    fs::write(&tmp, format!("{json}\n")).with_context(|| format!("writing {}", tmp.display()))?;

    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;

    Ok(())
}

fn current_profile(snapshot: &cerynth_telemetry::SystemSnapshot) -> Profile {
    snapshot
        .scheduler_profile
        .as_deref()
        .and_then(|value| value.parse::<Profile>().ok())
        .unwrap_or(Profile::Balanced)
}

fn main() -> Result<()> {
    println!("Starting Cerynth adaptive shadow policy...");

    let path = output_path();

    println!("Policy output : {}", path.display());
    println!("Apply         : false");
    println!("Interval      : 1s");

    let mut telemetry = ProcTelemetrySource::new();
    let mut policy = RulesBasedPolicy::new();

    loop {
        let output = match telemetry.collect() {
            Ok(snapshot) => {
                let input = PolicyInput {
                    cpu_usage_percent: snapshot.cpu_usage_percent,
                    load_1m: snapshot.load_1m,
                    runnable_tasks: snapshot.runnable_tasks,
                    context_switch_rate: snapshot.context_switch_rate,
                    short_lived_process_rate: snapshot.short_lived_process_rate,
                    current_profile: current_profile(&snapshot),
                };

                let decision = policy.evaluate(&input);

                println!(
                    "POLICY_FINAL cpu={:.2} load={:.2} runnable={} ctxt_rate={:.2} short_lived={:.2} profile={:?} => {:?} conf={:.2}",
                    input.cpu_usage_percent,
                    input.load_1m,
                    input.runnable_tasks,
                    input.context_switch_rate,
                    input.short_lived_process_rate,
                    input.current_profile,
                    decision.recommended_profile,
                    decision.confidence,
                );

                PolicyOutput::from(decision)
            }

            Err(error) => PolicyOutput {
                recommended_profile: Profile::Balanced,
                confidence: 1.0,
                reason: format!("Telemetry collection failed; defaulting to balanced: {error}"),
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                apply: false,
            },
        };

        if let Err(error) = write_policy(&output, &path) {
            eprintln!("Failed to write policy recommendation: {error}");
        }

        thread::sleep(Duration::from_secs(1));
    }
}
