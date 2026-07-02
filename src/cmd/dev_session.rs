//! `penv dev-session [--session <name>] [--preset <name>] [--attach]`
//!
//! Launches a tmux session defined by the `sessions` array in settings.json.
//! Each tab becomes one tmux window; panes are placed in at most a 2-column
//! grid.  Grid positions not covered by a configured pane become idle shells.
use anyhow::{bail, Result};

use crate::cmd::pick_preset::resolve_backend_preset;
use crate::cmd::worktree::write_close_session_script;
use crate::config::{repo_parent, repo_root, Settings};
use crate::tmux::{
    build_tab_panes, exec_tmux_attach, first_pane_id, sessions_with_prefix,
    setup_claude_window, tmux, tmux_send_keys, tmux_session_exists,
};

// ── Public types ───────────────────────────────────────────────────────────────

pub struct DevSessionArgs {
    pub session_name: Option<String>,
    pub preset: Option<String>,
    pub attach: bool,
}

// ── run() ─────────────────────────────────────────────────────────────────────

pub fn run(args: &DevSessionArgs, settings: &Settings) -> Result<()> {
    let root = repo_parent();
    let script_dir = repo_root();
    let penv = format!("{}/penv", script_dir.to_string_lossy());

    let sessions_cfg = &settings.project.sessions;
    if sessions_cfg.is_empty() {
        bail!("No sessions configured in settings.json under 'sessions'");
    }

    let session_cfg = if let Some(name) = &args.session_name {
        sessions_cfg
            .iter()
            .find(|s| s.session_name == *name)
            .ok_or_else(|| anyhow::anyhow!("Session '{}' not found in config", name))?
    } else if sessions_cfg.len() == 1 {
        &sessions_cfg[0]
    } else {
        bail!(
            "Multiple sessions configured. Specify one with --session <name>:\n{}",
            sessions_cfg
                .iter()
                .map(|s| format!("  {}", s.session_name))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let session = &session_cfg.session_name;

    if args.attach {
        let mut existing = sessions_with_prefix(session);
        existing.sort();
        let target = match existing.len() {
            0 => bail!("No running session found with prefix '{}'", session),
            1 => existing.remove(0),
            _ => crate::cmd::worktree::pick_session_fzf(&existing)?,
        };
        eprintln!("Attaching to '{}'...", target);
        exec_tmux_attach(&target);
    }

    if session_cfg.tabs.is_empty() {
        bail!("Session '{}' has no tabs configured", session);
    }

    if tmux_session_exists(session) {
        eprintln!("Session '{}' already exists. Attaching...", session);
        exec_tmux_attach(session);
    }

    let backend_box = crate::backend::active_backend(settings);
    let bref = backend_box.as_deref();

    let preset = args
        .preset
        .clone()
        .map(Ok)
        .unwrap_or_else(|| resolve_backend_preset(None, bref, settings))?;

    // Collect unique service names across all tabs for sync check + helper script.
    let all_services: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        session_cfg
            .tabs
            .iter()
            .flat_map(|t| t.panes.iter())
            .filter(|p| !p.repo.is_empty())
            .filter(|p| seen.insert(p.repo.clone()))
            .map(|p| p.repo.clone())
            .collect()
    };

    let sync_items: Vec<(&str, Option<&str>)> =
        all_services.iter().map(|s| (s.as_str(), None)).collect();
    crate::cmd::morning_check::startup_sync_check(&sync_items, &preset, bref);
    println!();

    let root_s = root.to_string_lossy().into_owned();
    let default_dir = root_s.as_str();

    // ── Build each tab ────────────────────────────────────────────────────────

    // First idle-shell pane found in any tab — receives the helper script.
    let mut helper_pane: Option<String> = None;

    for (tab_idx, tab) in session_cfg.tabs.iter().enumerate() {
        let n_rows = tab.panes.iter().map(|p| p.row + 1).max().unwrap_or(1);
        let n_cols = tab.panes.iter().map(|p| p.col + 1).max().unwrap_or(1).min(2);

        let tab_name = if tab.name.is_empty() {
            if tab_idx == 0 {
                "main".to_string()
            } else {
                format!("tab{}", tab_idx)
            }
        } else {
            tab.name.clone()
        };

        if tab_idx == 0 {
            tmux(&["new-session", "-d", "-s", session, "-n", &tab_name, "-c", default_dir])?;
        } else {
            tmux(&["new-window", "-t", session, "-n", &tab_name, "-c", default_dir])?;
        }

        let first = first_pane_id(session, tab_idx as u32)?;
        let grid = build_tab_panes(&first, n_rows, n_cols, default_dir)?;

        // Find the first grid position with no non-empty cmd → idle shell candidate.
        if helper_pane.is_none() {
            'outer: for r in 0..n_rows {
                for c in 0..n_cols {
                    let has_cmd = tab
                        .panes
                        .iter()
                        .any(|p| p.row == r && p.col == c && !p.cmd.is_empty());
                    if !has_cmd {
                        helper_pane = Some(grid[r][c].clone());
                        break 'outer;
                    }
                }
            }
        }

        // Send commands to configured panes.
        for pane_cfg in &tab.panes {
            if pane_cfg.cmd.is_empty() {
                continue;
            }
            let row = pane_cfg.row.min(n_rows - 1);
            let col = pane_cfg.col.min(n_cols - 1);
            let pane_id = &grid[row][col];

            let dir = if pane_cfg.repo.is_empty() {
                default_dir.to_string()
            } else {
                root.join(&pane_cfg.repo).to_string_lossy().into_owned()
            };
            tmux_send_keys(pane_id, &format!("cd '{}' && {}", dir, pane_cfg.cmd))?;
        }
    }

    // ── Helper script in idle shell ───────────────────────────────────────────

    if let Some(pane_id) = &helper_pane {
        let helper_path = format!("/tmp/dev-session-helper-{}", session);
        let svc_refs: Vec<&str> = all_services.iter().map(|s| s.as_str()).collect();
        write_close_session_script(&helper_path, session, &penv, &preset, &svc_refs)?;
        let services_arg = all_services
            .iter()
            .map(|s| format!("'{}'", s))
            .collect::<Vec<_>>()
            .join(" ");
        tmux_send_keys(
            pane_id,
            &format!(
                "'{}' show-info --preset '{}' --services {} --notes 'reload_env    — reload .env from current preset' 'close_session — close session' ||:; source '{}'; rm -f '{}'",
                penv, preset, services_arg, helper_path, helper_path,
            ),
        )?;
    }

    // ── Claude window + attach ────────────────────────────────────────────────

    setup_claude_window(session, &settings.project.claude)?;

    tmux(&["select-window", "-t", &format!("{}:0", session)])?;
    exec_tmux_attach(session);
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::config::{GridPaneConfig, TabConfig};

    fn pane(repo: &str, cmd: &str, col: usize, row: usize) -> GridPaneConfig {
        GridPaneConfig {
            repo: repo.to_string(),
            cmd: cmd.to_string(),
            col,
            row,
        }
    }

    #[test]
    fn grid_dims_single_pane() {
        let tab = TabConfig {
            name: "main".to_string(),
            panes: vec![pane("my-api", "npm run dev", 0, 0)],
        };
        let n_rows = tab.panes.iter().map(|p| p.row + 1).max().unwrap_or(1);
        let n_cols = tab.panes.iter().map(|p| p.col + 1).max().unwrap_or(1).min(2);
        assert_eq!((n_rows, n_cols), (1, 1));
    }

    #[test]
    fn grid_dims_2x2() {
        let tab = TabConfig {
            name: "main".to_string(),
            panes: vec![
                pane("my-api", "npm run dev", 0, 0),
                pane("my-ui",  "npm run dev", 0, 1),
                pane("my-ui",  "npm run sb",  1, 1),
            ],
        };
        let n_rows = tab.panes.iter().map(|p| p.row + 1).max().unwrap_or(1);
        let n_cols = tab.panes.iter().map(|p| p.col + 1).max().unwrap_or(1).min(2);
        assert_eq!((n_rows, n_cols), (2, 2));
    }

    #[test]
    fn grid_dims_col_capped_at_2() {
        let tab = TabConfig {
            name: "main".to_string(),
            panes: vec![pane("svc", "cmd", 5, 0)],
        };
        let n_cols = tab.panes.iter().map(|p| p.col + 1).max().unwrap_or(1).min(2);
        assert_eq!(n_cols, 2);
    }

    #[test]
    fn helper_pane_is_first_unconfigured_position() {
        let tab = TabConfig {
            name: "main".to_string(),
            panes: vec![
                pane("my-api", "npm run dev", 0, 0),
                // (0,1) and (1,0) unconfigured → (0,1) wins (row=0 first)
                pane("my-ui",  "npm run sb",  1, 1),
            ],
        };
        let n_rows = tab.panes.iter().map(|p| p.row + 1).max().unwrap_or(1);
        let n_cols = tab.panes.iter().map(|p| p.col + 1).max().unwrap_or(1).min(2);
        assert_eq!((n_rows, n_cols), (2, 2));

        let mut helper: Option<(usize, usize)> = None;
        'outer: for r in 0..n_rows {
            for c in 0..n_cols {
                let has_cmd = tab.panes.iter().any(|p| p.row == r && p.col == c && !p.cmd.is_empty());
                if !has_cmd {
                    helper = Some((r, c));
                    break 'outer;
                }
            }
        }
        assert_eq!(helper, Some((0, 1)));
    }

    #[test]
    fn helper_pane_none_when_all_panes_have_cmds() {
        let tab = TabConfig {
            name: "main".to_string(),
            panes: vec![
                pane("a", "cmd1", 0, 0),
                pane("b", "cmd2", 1, 0),
            ],
        };
        let n_rows = tab.panes.iter().map(|p| p.row + 1).max().unwrap_or(1);
        let n_cols = tab.panes.iter().map(|p| p.col + 1).max().unwrap_or(1).min(2);

        let mut helper: Option<(usize, usize)> = None;
        'outer: for r in 0..n_rows {
            for c in 0..n_cols {
                let has_cmd = tab.panes.iter().any(|p| p.row == r && p.col == c && !p.cmd.is_empty());
                if !has_cmd { helper = Some((r, c)); break 'outer; }
            }
        }
        assert_eq!(helper, None);
    }
}
