//! `penv open-pr [--branch <name>]`
//!
//! Find the GitHub PR for the current (or specified) branch using `gh pr view`
//! and open it in the default browser.  Exits with an error if no PR exists.
use anyhow::{Context, Result, bail};

use crate::cmd::open_ticket::open_url;

pub fn run(branch_override: Option<&str>) -> Result<()> {
    let mut cmd = std::process::Command::new("gh");
    cmd.args(["pr", "view", "--json", "url", "--jq", ".url"]);
    if let Some(b) = branch_override {
        cmd.arg(b);
    }

    let out = cmd
        .output()
        .context("failed to run 'gh' — is the GitHub CLI installed and authenticated?")?;

    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();

    if !out.status.success() || url.is_empty() {
        let branch_label = branch_override.unwrap_or("current branch");
        bail!("No PR found for {}.", branch_label);
    }

    eprintln!("Opening PR — {}", url);
    open_url(&url)
}
