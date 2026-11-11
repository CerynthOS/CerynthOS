//! Trusted checkout helper. Run only inside the manager's isolated workspace.
//! Local Git configuration is data: filters and hooks must never execute.
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeSet,
    process::{Command, Stdio},
};

fn git() -> Command {
    let mut command = Command::new("/usr/bin/git");
    command.args([
        "--no-pager",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "submodule.recurse=false",
        "-c",
        "core.sparseCheckout=false",
        "-c",
        "gc.auto=0",
        "-c",
        "maintenance.auto=false",
    ]);
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_NO_LAZY_FETCH", "1");
    command
}

fn main() -> Result<()> {
    ensure!(
        std::env::var("CERYNTH_WORKTREE_CHECKOUT").as_deref() == Ok("1"),
        "this helper requires an isolated manager launch"
    );
    let output = git()
        .args([
            "config",
            "--includes",
            "--null",
            "--name-only",
            "--get-regexp",
            "^filter\\.",
        ])
        .stderr(Stdio::null())
        .output()?;
    ensure!(
        output.status.success() || output.status.code() == Some(1),
        "cannot inspect checkout filters"
    );
    ensure!(
        output.stdout.len() <= 65536,
        "filter metadata exceeds limit"
    );
    let mut filters = BTreeSet::new();
    for key in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|key| !key.is_empty())
    {
        let key = std::str::from_utf8(key)?;
        let rest = key.strip_prefix("filter.").context("invalid filter key")?;
        let (name, _) = rest.rsplit_once('.').context("invalid filter key")?;
        ensure!(
            !name.is_empty()
                && name.len() <= 128
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
            "unsupported filter name; prepare this workspace explicitly"
        );
        filters.insert(name.to_owned());
    }
    ensure!(filters.len() <= 128, "too many checkout filters");
    let mut checkout = git();
    for name in filters {
        for kind in ["clean", "smudge", "process"] {
            checkout.args(["-c", &format!("filter.{name}.{kind}=")]);
        }
        checkout.args(["-c", &format!("filter.{name}.required=false")]);
    }
    let status = checkout
        .args(["read-tree", "--reset", "-u", "HEAD"])
        .stdin(Stdio::null())
        .status()?;
    ensure!(status.success(), "isolated checkout failed");
    Ok(())
}
