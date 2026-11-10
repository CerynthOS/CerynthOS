//! Prints the cgroup v2 resource support matrix for this host.
//!
//! `cerynth-resource-support` is deliberately read-only apart from creating a
//! temporary child cgroup when the probe root is writable. It never applies a
//! limit to a real session.

use std::path::PathBuf;

use cerynth_resources::{CgroupRoot, SupportMatrix};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "cerynth-resource-support",
    about = "Probe which cgroup v2 resource controls this host can enforce"
)]
struct Args {
    /// cgroup v2 root to probe. Defaults to a writable root for this user.
    #[arg(long)]
    root: Option<PathBuf>,

    /// Emit the machine-readable support matrix instead of a table.
    #[arg(long)]
    json: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let root = match args.root {
        Some(path) => CgroupRoot::at(path)?,
        None => CgroupRoot::detect_writable()?,
    };
    let matrix = SupportMatrix::probe(&root);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&matrix)?);
    } else {
        print_human(&matrix);
    }
    Ok(())
}

fn print_human(matrix: &SupportMatrix) {
    println!(
        "CerynthOS resource support matrix (schema {})",
        matrix.schema_version
    );
    println!("cgroup v2:       {}", yes_no(matrix.cgroup_v2));
    println!("mount:           {}", matrix.mount);
    println!(
        "probe root:      {}{}",
        matrix.probe_root,
        if matrix.probed_in_child {
            " (temporary child)"
        } else {
            " (in place)"
        }
    );
    println!("writable:        {}", yes_no(matrix.writable));
    println!("controllers:     {}", join_or_none(&matrix.controllers));
    println!("subtree_control: {}", join_or_none(&matrix.subtree_control));
    println!(
        "PSI:             cpu={} memory={} io={}",
        yes_no(matrix.psi.cpu),
        yes_no(matrix.psi.memory),
        yes_no(matrix.psi.io)
    );
    println!("cgroup.kill:     {}", yes_no(matrix.cgroup_kill));
    println!("memory.peak:     {}", yes_no(matrix.memory_peak));
    if let Some(systemd) = &matrix.systemd {
        println!(
            "systemd:         {} (user scope: {})",
            systemd.version.as_deref().unwrap_or("unknown"),
            yes_no(systemd.user_scope)
        );
    }

    println!();
    println!(
        "{:<14} {:<11} {:<14} {:<8} {:<9} {:<12} NOTE",
        "LIMIT", "CONTROLLER", "FILE", "PRESENT", "WRITABLE", "ENFORCEABLE"
    );
    for limit in &matrix.limits {
        println!(
            "{:<14} {:<11} {:<14} {:<8} {:<9} {:<12} {}",
            limit.name,
            limit.controller,
            limit.file,
            yes_no(limit.present),
            yes_no(limit.writable),
            yes_no(limit.enforceable),
            limit.note.as_deref().unwrap_or("")
        );
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "(none)".to_string()
    } else {
        values.join(" ")
    }
}
