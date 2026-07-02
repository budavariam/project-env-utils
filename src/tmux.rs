//! Shared tmux helpers used by dev_ui and dev_backend.
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

// ── Core helpers ───────────────────────────────────────────────────────────────

/// Run a tmux command, returning an error if it fails.
pub fn tmux(args: &[&str]) -> Result<()> {
    let status = Command::new("tmux")
        .args(args)
        .status()
        .with_context(|| format!("failed to spawn tmux {:?}", args))?;
    if !status.success() {
        bail!("tmux {:?} exited with {}", args, status);
    }
    Ok(())
}

/// Run a tmux command capturing stdout; returns trimmed output.
pub fn tmux_output(args: &[&str]) -> Result<String> {
    let out = Command::new("tmux")
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn tmux {:?}", args))?;
    if !out.status.success() {
        bail!(
            "tmux {:?} exited with {}: {}",
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Send keys to a tmux target pane, appending Enter.
pub fn tmux_send_keys(target: &str, cmd: &str) -> Result<()> {
    tmux(&["send-keys", "-t", target, cmd, "Enter"])
}

/// Return true if a tmux session with the given name exists.
pub fn tmux_session_exists(name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", &format!("={}", name)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Return all running tmux session names whose name starts with `prefix`.
/// Returns an empty vec if tmux isn't running or no sessions exist.
pub fn sessions_with_prefix(prefix: &str) -> Vec<String> {
    let out = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|s| s.starts_with(prefix))
            .map(|s| s.to_string())
            .collect(),
        _ => vec![],
    }
}

/// Replace the current process with `tmux attach-session -t =<session>`.
/// Never returns on success.
pub fn exec_tmux_attach(session: &str) -> ! {
    let err = Command::new("tmux")
        .args(["attach-session", "-t", &format!("={}", session)])
        .exec(); // replaces process; only returns on error
    eprintln!("Failed to attach to tmux session: {}", err);
    std::process::exit(1);
}

// ── Pane helpers ───────────────────────────────────────────────────────────────

/// Parse `tmux list-panes -F "#{pane_id} #{pane_left} #{pane_top}"` output.
/// Returns Vec<(pane_id, left, top)> sorted by (left ASC, top ASC).
pub fn parse_panes(output: &str) -> Vec<(String, u32, u32)> {
    let mut panes: Vec<(String, u32, u32)> = output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let id = parts[0].to_string();
                let left = parts[1].parse::<u32>().ok()?;
                let top = parts[2].parse::<u32>().ok()?;
                Some((id, left, top))
            } else {
                None
            }
        })
        .collect();
    panes.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
    panes
}

/// List panes in a session and return sorted Vec<(id, left, top)>.
pub fn list_panes(session: &str) -> Result<Vec<(String, u32, u32)>> {
    let out = tmux_output(&[
        "list-panes",
        "-t",
        &format!("={}", session),
        "-F",
        "#{pane_id} #{pane_left} #{pane_top}",
    ])?;
    Ok(parse_panes(&out))
}

/// Get the first pane ID of a window by index.
pub fn first_pane_id(session: &str, window_index: u32) -> Result<String> {
    let target = format!("{}:{}.0", session, window_index);
    tmux_output(&["display-message", "-p", "-t", &target, "#{pane_id}"])
}

// ── Session setup helpers ──────────────────────────────────────────────────────

/// Create (or reuse) a named Claude tmux session and link its window into the
/// given dev session.  When `claude_cfg` is disabled this is a no-op.
pub fn setup_claude_window(
    session: &str,
    claude_cfg: &crate::config::ClaudeSessionConfig,
) -> Result<()> {
    if !claude_cfg.enabled {
        return Ok(());
    }

    let session_name = "claude";
    let start_dir = if claude_cfg.start_dir.is_empty() {
        ".".to_string()
    } else if claude_cfg.start_dir.starts_with("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}{}", home, &claude_cfg.start_dir[1..])
    } else {
        claude_cfg.start_dir.clone()
    };

    if !tmux_session_exists(session_name) {
        tmux(&[
            "new-session",
            "-d",
            "-s",
            session_name,
            "-n",
            "claude",
            "-c",
            &start_dir,
        ])?;
        tmux_send_keys(&format!("{}:claude", session_name), "claude")?;
    }
    tmux(&[
        "link-window",
        "-s",
        &format!("{}:claude", session_name),
        "-t",
        &format!("{}:", session),
    ])
}

// ── Grid layout ────────────────────────────────────────────────────────────────

/// Build a 2-column grid of panes for one tmux window.
///
/// `first_pane` is the pane ID that already exists (created by `new-session` or
/// `new-window`).  The function splits it into the requested `n_rows × n_cols`
/// grid and returns `grid[row][col]` pane IDs.
///
/// Grid positions not wired to a command by the caller become idle shells —
/// they are created but never receive `send-keys`.
///
/// # Layout
/// ```text
/// n_cols=1: left column only, rows split vertically.
/// n_cols=2: left column and right column, each row split vertically.
///
///   | col 0   | col 1   |
///   | row 0   | row 0   |   ← first_pane + 1 h-split
///   | row 1   | row 1   |   ← 2 v-splits
///   | row 2   | row 2   |   ← 2 v-splits …
/// ```
pub fn build_tab_panes(
    first_pane: &str,
    n_rows: usize,
    n_cols: usize,
    default_dir: &str,
) -> Result<Vec<Vec<String>>> {
    let n_cols = n_cols.max(1).min(2);
    let n_rows = n_rows.max(1);

    let mut grid = vec![vec![String::new(); n_cols]; n_rows];
    grid[0][0] = first_pane.to_string();

    if n_cols == 2 {
        let p = tmux_output(&[
            "split-window", "-h", "-l", "50%", "-t", first_pane,
            "-c", default_dir, "-P", "-F", "#{pane_id}",
        ])?;
        if p.is_empty() { anyhow::bail!("build_tab_panes: h-split returned empty pane id"); }
        grid[0][1] = p;
    }

    for row in 1..n_rows {
        let prev_left = grid[row - 1][0].clone();
        let p = tmux_output(&[
            "split-window", "-v", "-l", "50%", "-t", &prev_left,
            "-c", default_dir, "-P", "-F", "#{pane_id}",
        ])?;
        if p.is_empty() { anyhow::bail!("build_tab_panes: v-split (col 0, row {}) returned empty pane id", row); }
        grid[row][0] = p;

        if n_cols == 2 {
            let prev_right = grid[row - 1][1].clone();
            let p = tmux_output(&[
                "split-window", "-v", "-l", "50%", "-t", &prev_right,
                "-c", default_dir, "-P", "-F", "#{pane_id}",
            ])?;
            if p.is_empty() { anyhow::bail!("build_tab_panes: v-split (col 1, row {}) returned empty pane id", row); }
            grid[row][1] = p;
        }
    }

    Ok(grid)
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_panes_sorts_by_left_then_top() {
        let input = "%2 0 15\n%0 0 0\n%1 90 0\n";
        let panes = parse_panes(input);
        assert_eq!(panes[0].0, "%0"); // left=0, top=0
        assert_eq!(panes[1].0, "%2"); // left=0, top=15
        assert_eq!(panes[2].0, "%1"); // left=90, top=0
    }

    #[test]
    fn parse_panes_empty_input() {
        let panes = parse_panes("");
        assert!(panes.is_empty());
    }

    #[test]
    fn parse_panes_skips_malformed_lines() {
        let input = "%0 0 0\nbad line\n%1 10 5\n";
        let panes = parse_panes(input);
        assert_eq!(panes.len(), 2);
    }
}
