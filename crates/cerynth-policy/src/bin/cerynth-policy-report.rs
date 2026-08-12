//! Summarize adaptive policy replay results across recorded workloads.
//!
//! This tool is read-only: it analyzes policy decisions produced by
//! `cerynth-policy-replay` and never modifies the scheduler.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cerynth_ipc::Profile;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PolicyRecord {
    current_profile: Profile,
    recommended_profile: Profile,
    confidence: f64,
    reason: String,
}

#[derive(Debug, Default)]
struct Report {
    samples: usize,
    recommendation_counts: BTreeMap<String, usize>,
    transitions: BTreeMap<String, usize>,
    dwell_holds: usize,
    cooldown_holds: usize,
    first: Option<Profile>,
    last: Option<Profile>,
}

fn profile_name(profile: Profile) -> String {
    profile.to_string()
}

fn analyze(path: &Path) -> Result<Report> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;

    let mut report = Report::default();
    let mut previous: Option<Profile> = None;

    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let record: PolicyRecord =
            serde_json::from_str(line).with_context(|| {
                format!("parsing policy decision in {}", path.display())
            })?;

        report.samples += 1;

        let recommended = record.recommended_profile;

        *report
            .recommendation_counts
            .entry(profile_name(recommended))
            .or_default() += 1;

        if report.first.is_none() {
            report.first = Some(recommended);
        }

        report.last = Some(recommended);

        if let Some(previous_profile) = previous {
            if previous_profile != recommended {
                let transition = format!(
                    "{} -> {}",
                    profile_name(previous_profile),
                    profile_name(recommended)
                );

                *report.transitions.entry(transition).or_default() += 1;
            }
        }

        if record.reason.contains("consecutive samples") {
            report.dwell_holds += 1;
        }

        if record.reason.contains("cooldown") {
            report.cooldown_holds += 1;
        }

        // Keep the field intentionally deserialized because confidence is
        // part of the replay record and may be useful for future reporting.
        let _confidence = record.confidence;
        let _current_profile = record.current_profile;

        previous = Some(recommended);
    }

    Ok(report)
}

fn print_report(path: &Path, report: &Report) {
    println!();
    println!("========================================");
    println!("Workload: {}", path.display());
    println!("========================================");

    println!("Samples: {}", report.samples);

    if let Some(first) = report.first {
        println!("First recommendation: {}", first);
    }

    if let Some(last) = report.last {
        println!("Last recommendation:  {}", last);
    }

    println!();
    println!("Recommendations:");

    for (profile, count) in &report.recommendation_counts {
        println!("  {profile}: {count}");
    }

    println!();
    println!("Transitions:");

    if report.transitions.is_empty() {
        println!("  none");
    } else {
        for (transition, count) in &report.transitions {
            println!("  {transition}: {count}");
        }
    }

    println!();
    println!("Stability:");

    println!("  dwell holds:    {}", report.dwell_holds);
    println!("  cooldown holds: {}", report.cooldown_holds);
}

fn main() -> Result<()> {
    let artifacts = PathBuf::from("artifacts");

    let mut files: Vec<PathBuf> = fs::read_dir(&artifacts)
        .with_context(|| format!("reading {}", artifacts.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("policy-decisions-")
                        && name.ends_with(".jsonl")
                })
        })
        .collect();

    files.sort();

    if files.is_empty() {
        println!("No policy replay files found in artifacts/");
        return Ok(());
    }

    println!("CerynthOS Adaptive Policy Replay Report");
    println!("========================================");

    for path in files {
        let report = analyze(&path)?;
        print_report(&path, &report);
    }

    Ok(())
}
