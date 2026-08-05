//! Records periodic [`SystemSnapshot`]s to a JSON Lines file.
//!
//! ```text
//! cerynth-telemetry-record --interval-ms 1000 --duration-seconds 60 --output data/run.jsonl
//! ```

use std::fs::{OpenOptions, create_dir_all};
use std::io::Write;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use anyhow::Result;
use cerynth_telemetry::{ProcTelemetrySource, TelemetrySource};
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "cerynth-telemetry-record",
    about = "Record CerynthOS system telemetry to JSONL"
)]
struct Args {
    #[arg(long, default_value_t = 1000)]
    interval_ms: u64,

    #[arg(long, default_value_t = 60)]
    duration_seconds: u64,

    #[arg(long, default_value = "data/run.jsonl")]
    output: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(parent) = args.output.parent().filter(|p| !p.as_os_str().is_empty()) {
        create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.output)?;

    let interval = Duration::from_millis(args.interval_ms.max(1));
    let iterations = ((args.duration_seconds * 1000) / args.interval_ms.max(1)).max(1);

    let mut source = ProcTelemetrySource::new();
    // The first sample has nothing to diff CPU usage against, so take it
    // as a throwaway baseline before the recorded loop starts.
    source.collect()?;
    sleep(interval);

    for i in 0..iterations {
        let snapshot = source.collect()?;
        writeln!(file, "{}", serde_json::to_string(&snapshot)?)?;
        file.flush()?;
        eprintln!(
            "[{}/{}] cpu={:.1}% load1={:.2} sched_ext={} profile={}",
            i + 1,
            iterations,
            snapshot.cpu_usage_percent,
            snapshot.load_1m,
            snapshot.sched_ext_state,
            snapshot.scheduler_profile.as_deref().unwrap_or("unknown"),
        );
        if i + 1 < iterations {
            sleep(interval);
        }
    }

    Ok(())
}
