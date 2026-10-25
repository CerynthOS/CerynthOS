//! Summarize adaptive policy replay results across recorded workloads.
//!
//! This tool is read-only: it analyzes policy decisions produced by
//! `cerynth-policy-replay` and never modifies the scheduler.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cerynth_ipc::Profile;
use clap::Parser;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PolicyRecord {
    #[serde(default)]
    current_profile: Option<Profile>,
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
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

    let mut report = Report::default();
    let mut previous: Option<Profile> = None;

    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let record: PolicyRecord = serde_json::from_str(line)
            .with_context(|| format!("parsing policy decision in {}", path.display()))?;

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
    println!("Recommendation transitions:");

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

#[derive(Parser)]
#[command(about = "Summarize policy decision JSONL files")]
struct Args {
    #[arg(
        long,
        required_unless_present = "directory",
        conflicts_with = "directory"
    )]
    input: Option<PathBuf>,
    #[arg(long)]
    directory: Option<PathBuf>,
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(directory).with_context(|| format!("reading {}", directory.display()))?
    {
        let path = entry?.path();
        if path.is_dir() && !path.is_symlink() {
            collect_files(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            files.push(path);
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut files = Vec::new();
    if let Some(input) = args.input {
        files.push(input);
    } else if let Some(directory) = args.directory {
        collect_files(&directory, &mut files)?;
    }
    files.sort();
    anyhow::ensure!(!files.is_empty(), "No policy JSONL files found");
    println!("CerynthOS Adaptive Policy Replay Report");
    for path in files {
        print_report(&path, &analyze(&path)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_exactly_one_source() {
        assert!(Args::try_parse_from(["report"]).is_err());
        assert!(Args::try_parse_from(["report", "--input", "a", "--directory", "b"]).is_err());
        assert!(Args::try_parse_from(["report", "--input", "a"]).is_ok());
        assert!(Args::try_parse_from(["report", "--directory", "b"]).is_ok());
    }

    #[test]
    fn live_records_need_no_simulated_current_profile() {
        let record: PolicyRecord = serde_json::from_str(
            r#"{"recommended_profile":"interactive","confidence":0.84,"reason":"churn","apply":false}"#,
        ).unwrap();
        assert!(record.current_profile.is_none());
        assert_eq!(record.recommended_profile, Profile::Interactive);
    }
}
