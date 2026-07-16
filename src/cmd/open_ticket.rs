//! `penv open-ticket [--branch <name>]`
//!
//! Extract a ticket ID from the current git branch (or --branch argument) using
//! the pattern configured in `ticket_manager.pattern`, then open the configured
//! `ticket_manager.url_template` in the default browser.
use anyhow::{Context, Result, bail};
use regex::Regex;

use crate::config::Settings;

pub fn run(branch_override: Option<&str>, settings: &Settings) -> Result<()> {
    let tm = &settings.project.ticket_manager;
    if !tm.is_configured() {
        bail!(
            "No ticket_manager configured in settings.json.\n\
             Add a 'ticket_manager' block with 'pattern' and 'url_template' fields."
        );
    }

    let branch = if let Some(b) = branch_override {
        b.to_string()
    } else {
        let out = std::process::Command::new("git")
            .args(["symbolic-ref", "--short", "HEAD"])
            .output()
            .context("git symbolic-ref failed")?;
        if !out.status.success() {
            bail!("Could not determine current git branch. Are you inside a git repo?");
        }
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    let re = Regex::new(&tm.pattern)
        .with_context(|| format!("Invalid ticket_manager.pattern: {}", tm.pattern))?;

    let ticket = re
        .find(&branch)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No ticket ID found in branch '{}' using pattern '{}'",
                branch,
                tm.pattern
            )
        })?
        .as_str()
        .to_string();

    let url = tm.url_template.replace("{ticket}", &ticket);
    eprintln!("Opening ticket {} — {}", ticket, url);
    open_url(&url)
}

/// Extract a ticket ID from `name` using `pattern`, or return None.
pub fn extract_ticket(pattern: &str, name: &str) -> Option<String> {
    Regex::new(pattern)
        .ok()?
        .find(name)
        .map(|m| m.as_str().to_string())
}

fn open_url(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let cmd = "open";

    std::process::Command::new(cmd)
        .arg(url)
        .status()
        .with_context(|| format!("failed to launch '{}' to open URL", cmd))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_wrike_numeric() {
        assert_eq!(
            extract_ticket(r"\d+", "feat/4494658262-add-feature"),
            Some("4494658262".to_string())
        );
    }

    #[test]
    fn extract_jira_format() {
        assert_eq!(
            extract_ticket(r"[A-Z]+-\d+", "feat/ABC-123-some-fix"),
            Some("ABC-123".to_string())
        );
    }

    #[test]
    fn extract_no_match_returns_none() {
        assert_eq!(extract_ticket(r"\d+", "main"), None);
    }

    #[test]
    fn extract_safe_branch_name() {
        // branch_to_safe replaces / with _ — pattern still works
        assert_eq!(
            extract_ticket(r"\d+", "feat_4494658262-my-feature"),
            Some("4494658262".to_string())
        );
    }
}
