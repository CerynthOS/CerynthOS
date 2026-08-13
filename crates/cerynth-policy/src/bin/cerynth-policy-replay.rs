//! Replay recorded `CerynthOS` telemetry through the adaptive policy.
//!
//! This tool is intentionally shadow-only: it produces recommendations but
//! never modifies the running scheduler.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use cerynth_ipc::Profile;
use cerynth_policy::{Policy, PolicyDecision, PolicyInput, RulesBasedPolicy};
use cerynth_telemetry::SystemSnapshot;
use clap::Parser;
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(
    name = "cerynth-policy-replay",
    about = "Replay telemetry through the CerynthOS adaptive policy"
)]
struct Args {
    /// Input telemetry JSONL file.
    #[arg(long)]
    input: PathBuf,

    /// Output policy decisions JSONL file.
    #[arg(long, default_value = "artifacts/policy-decisions.jsonl")]
    output: PathBuf,
}

#[derive(Debug, Serialize)]
struct PolicyRecord {
    timestamp_ms: u128,
    current_profile: Profile,
    recommended_profile: Profile,
    confidence: f64,
    reason: String,
}

fn profile_from_snapshot(snapshot: &SystemSnapshot) -> Profile {
    snapshot
        .scheduler_profile
        .as_deref()
        .and_then(|profile| profile.parse().ok())
        .unwrap_or(Profile::Balanced)
}

fn context_switch_rate(previous: Option<&SystemSnapshot>, current: &SystemSnapshot) -> f64 {
    let Some(previous) = previous else {
        return 0.0;
    };

    let elapsed_ms = current.timestamp_ms.saturating_sub(previous.timestamp_ms);

    if elapsed_ms == 0 {
        return 0.0;
    }

    let switches = current
        .context_switches
        .saturating_sub(previous.context_switches);

    #[allow(clippy::cast_precision_loss)]
    {
        switches as f64 / (elapsed_ms as f64 / 1000.0)
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let input = File::open(&args.input)
        .with_context(|| format!("opening input telemetry: {}", args.input.display()))?;

    if let Some(parent) = args.output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating output directory: {}", parent.display()))?;
    }

    let mut output = File::create(&args.output)
        .with_context(|| format!("creating output: {}", args.output.display()))?;

    let reader = BufReader::new(input);
    let mut previous: Option<SystemSnapshot> = None;
    let mut policy = RulesBasedPolicy::new();

    let mut records = 0usize;

    for (line_number, line) in reader.lines().enumerate() {
        let line_number = line_number + 1;
        let line = line.with_context(|| format!("reading telemetry line {line_number}"))?;

        if line.trim().is_empty() {
            continue;
        }

        let snapshot: SystemSnapshot = serde_json::from_str(&line)
            .with_context(|| format!("parsing telemetry JSON on line {line_number}"))?;

        let current_profile = profile_from_snapshot(&snapshot);

        let input = PolicyInput {
            cpu_usage_percent: snapshot.cpu_usage_percent,
            load_1m: snapshot.load_1m,
            runnable_tasks: snapshot.runnable_tasks,
            context_switch_rate: context_switch_rate(previous.as_ref(), &snapshot),

            // The current telemetry schema does not expose process lifetime
            // information, so this signal is unavailable in v1.
            short_lived_process_rate: 0.0,

            current_profile,
        };

        let decision: PolicyDecision = policy.evaluate(&input);

        let record = PolicyRecord {
            timestamp_ms: snapshot.timestamp_ms,
            current_profile,
            recommended_profile: decision.recommended_profile,
            confidence: decision.confidence,
            reason: decision.reason,
        };

        writeln!(output, "{}", serde_json::to_string(&record)?)?;

        previous = Some(snapshot);
        records += 1;
    }

    output.flush()?;

    println!("cerynth-policy-replay: processed {records} telemetry snapshots");
    println!(
        "cerynth-policy-replay: decisions written to {}",
        args.output.display()
    );

    Ok(())
}
