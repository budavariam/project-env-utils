//! `penv dev-session [--session <name>] [--preset <name>] [--attach]`
//!
//! Launches a tmux session defined by the `sessions` array in settings.json.
//! Each tab becomes one tmux window; panes are placed in at most a 2-column
//! grid.  Grid positions not covered by a configured pane become idle shells.
use anyhow::{Result, bail};

use crate::cmd::open_ticket::extract_ticket;
use crate::cmd::pick_preset::resolve_backend_preset;
use crate::cmd::show_info::SessionNotes;
use crate::cmd::worktree::{
    mode_from_flags, pick_workspace_mode, resolve_repo_workspace,
};
use crate::config::{Settings, repo_parent, repo_root};
use crate::env_file::sh_escape;
use crate::state::get_service_preset;
use crate::tmux::{active_mux, setup_linked_window};

// ── Public types ───────────────────────────────────────────────────────────────

pub struct DevSessionArgs {
    pub session_name: Option<String>,
    pub preset: Option<String>,
    pub attach: bool,
    /// Open repos in existing Claude worktrees.
    pub worktree: bool,
    /// Stash + checkout a branch in root repos.
    pub checkout: bool,
    /// Create/open Claude worktrees for repos.
    pub checkout_worktree: bool,
    /// Branch to use with --checkout or --checkout-worktree.
    pub branch: Option<String>,
    /// Interactively pick a workspace mode (overrides the other mode flags).
    pub mixed: bool,
}

// ── run() ─────────────────────────────────────────────────────────────────────

pub fn run(args: &DevSessionArgs, settings: &Settings) -> Result<()> {
    let mux = active_mux(settings);
    let root = repo_parent();
    let penv = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("{}/penv", repo_root().to_string_lossy()));

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
        let mut existing = mux.sessions_with_prefix(session);
        existing.sort();
        let target = match existing.len() {
            0 => bail!("No running session found with prefix '{}'", session),
            1 => existing.remove(0),
            _ => crate::cmd::worktree::pick_session_fzf(&existing)?,
        };
        eprintln!("Attaching to '{}'...", target);
        mux.attach(&target);
    }

    if session_cfg.tabs.is_empty() {
        bail!("Session '{}' has no tabs configured", session);
    }

    if mux.session_exists(session) {
        eprintln!("Session '{}' already exists. Attaching...", session);
        mux.attach(session);
    }

    let backend_box = crate::backend::active_backend(settings);
    let bref = backend_box.as_deref();

    // ── Preset resolution ─────────────────────────────────────────────────────
    // When --preset is given, use it for all services.
    // When there are multiple services and no --preset, prompt per service so
    // each repo can be on a different preset (unlike dev-ui/dev-backend which
    // only have one service each).

    // Collect unique services first (needed for preset resolution).
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

    // service → preset
    let service_presets: std::collections::HashMap<String, String> = if session_cfg.no_preset {
        let p = args.preset.clone().unwrap_or_default();
        all_services.iter().map(|s| (s.clone(), p.clone())).collect()
    } else if let Some(ref p) = args.preset {
        all_services.iter().map(|s| (s.clone(), p.clone())).collect()
    } else if all_services.len() <= 1 {
        // Single service: use the standard picker (same as dev-backend).
        let p = resolve_backend_preset(None, bref, settings)?;
        all_services.iter().map(|s| (s.clone(), p.clone())).collect()
    } else {
        // Multiple services without an explicit --preset: show numbered list per service.
        use std::io::{self, Write};
        let mut map = std::collections::HashMap::new();
        for svc in &all_services {
            let last = get_service_preset(svc);
            let avail = settings.project.available_presets_for_service(svc);
            let default = last
                .as_deref()
                .map(str::to_string)
                .or_else(|| avail.first().cloned())
                .unwrap_or_else(|| "dev".to_string());

            eprintln!("\n  Preset for {}:", svc);
            if avail.is_empty() {
                eprintln!("  (no presets found — type a name)");
            } else {
                for (i, p) in avail.iter().enumerate() {
                    let marker = if last.as_deref() == Some(p.as_str()) { "  ← last used" } else { "" };
                    eprintln!("    [{}] {}{}", i + 1, p, marker);
                }
            }
            let chosen = loop {
                eprint!("  Select [1-{}] or name (Enter = {}): ", avail.len().max(1), default);
                io::stderr().flush().ok();
                let mut line = String::new();
                io::stdin().read_line(&mut line).ok();
                let t = line.trim();
                if t.is_empty() {
                    break default.clone();
                }
                if let Ok(n) = t.parse::<usize>() {
                    if n >= 1 && n <= avail.len() {
                        break avail[n - 1].clone();
                    }
                }
                if !t.is_empty() {
                    break t.to_string();
                }
                eprintln!("  Invalid — enter a number or preset name.");
            };
            eprintln!("  {} → {}", svc, chosen);
            map.insert(svc.clone(), chosen);
        }
        map
    };

    // Canonical single preset for helper scripts (first service's, or common if all equal).
    let preset = service_presets
        .values()
        .next()
        .cloned()
        .unwrap_or_default();

    // ── Workspace mode ────────────────────────────────────────────────────────
    // Pre-resolve each unique repo's working directory.
    // --mixed: prompt independently per repo.
    // Other flags: apply the same mode to all repos.
    let global_mode = mode_from_flags(args.worktree, args.checkout, args.checkout_worktree, &args.branch);

    let repo_dirs: std::collections::HashMap<String, std::path::PathBuf> = {
        let mut map = std::collections::HashMap::new();
        for svc in &all_services {
            let repo_dir = root.join(svc);
            let mode = if args.mixed {
                eprintln!("\n[{}] workspace mode:", svc);
                pick_workspace_mode(&format!("{} mode> ", svc))?
            } else {
                global_mode.clone()
            };
            let resolved = resolve_repo_workspace(&repo_dir, &mode, args.branch.as_deref())
                .unwrap_or(repo_dir);
            map.insert(svc.clone(), resolved);
        }
        map
    };

    if !session_cfg.no_preset {
        for svc in &all_services {
            let svc_preset = service_presets.get(svc).map(String::as_str).unwrap_or(&preset);
            crate::cmd::morning_check::startup_sync_check(
                &[(svc.as_str(), None)],
                svc_preset,
                bref,
                settings,
            );
        }
        println!();
    }

    let root_s = root.to_string_lossy().into_owned();
    let default_dir = root_s.as_str();

    // ── Build each tab ────────────────────────────────────────────────────────

    // First idle-shell pane found in any tab — receives the helper script.
    let mut helper_pane: Option<String> = None;

    for (tab_idx, tab) in session_cfg.tabs.iter().enumerate() {
        let n_rows = tab.panes.iter().map(|p| p.row + 1).max().unwrap_or(1);
        let n_cols = tab
            .panes
            .iter()
            .map(|p| p.col + 1)
            .max()
            .unwrap_or(1)
            .min(2);

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
            mux.new_session(session, &tab_name, default_dir)?;
        } else {
            mux.new_window(session, &tab_name, default_dir)?;
        }

        let first = mux.first_pane_id(session, tab_idx as u32)?;
        let grid = mux.build_pane_grid(&first, n_rows, n_cols, default_dir)?;

        // Find the first grid position with no non-empty cmd → idle shell candidate.
        if helper_pane.is_none() {
            'outer: for (r, row) in grid.iter().enumerate() {
                for (c, pane_id) in row.iter().enumerate() {
                    let has_cmd = tab
                        .panes
                        .iter()
                        .any(|p| p.row == r && p.col == c && !p.cmd.is_empty());
                    if !has_cmd {
                        helper_pane = Some(pane_id.clone());
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
                repo_dirs
                    .get(&pane_cfg.repo)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.join(&pane_cfg.repo).to_string_lossy().into_owned())
            };
            if !pane_cfg.repo.is_empty() {
                mux.set_pane_title(pane_id, pane_cfg.pane_title()).ok();
            }
            mux.send_keys(
                pane_id,
                &format!("cd '{}' && {}", sh_escape(&dir), pane_cfg.cmd),
            )?;
        }
    }

    // ── Helper script in idle shell ───────────────────────────────────────────
    // Use the first idle pane found, or open a dedicated "shell" window when
    // the user's layout fills every grid position.

    let helper_pane: String = if let Some(pane_id) = helper_pane {
        pane_id
    } else {
        mux.new_window(session, "shell", default_dir)?;
        mux.first_pane_id(session, session_cfg.tabs.len() as u32)?
    };

    let helper_path = format!("/tmp/dev-session-helper-{}", session);
    let svc_refs: Vec<&str> = all_services.iter().map(|s| s.as_str()).collect();
    let services_arg = all_services
        .iter()
        .map(|s| format!("'{}'", s))
        .collect::<Vec<_>>()
        .join(" ");
    let message_arg = if session_cfg.message.is_empty() {
        String::new()
    } else {
        format!(
            " --message '{}'",
            session_cfg.message.replace('\'', "'\\''")
        )
    };

    // Build reload_cmd.
    // Multiple services: call change-preset per service so each gets its own
    // interactive preset picker when the user runs reload_env in the shell.
    // Single service: use the preset chosen at session start (standard behaviour).
    let reload_cmd_str: String = if all_services.len() > 1 {
        all_services
            .iter()
            .map(|svc| format!("'{}' change-preset '{}'", penv, svc))
            .collect::<Vec<_>>()
            .join(" && ")
    } else {
        all_services
            .iter()
            .map(|svc| {
                let svc_preset = service_presets.get(svc).map(String::as_str).unwrap_or(&preset);
                format!("'{}' load-env '{}' '{}'", penv, svc_preset, svc)
            })
            .collect::<Vec<_>>()
            .join(" && ")
    };
    let reload_cmd_override = if all_services.len() > 1 { Some(reload_cmd_str.as_str()) } else { None };

    let tm = &settings.project.ticket_manager;
    let has_ticket = tm.is_configured() && {
        // Check first service's current branch for a ticket ID.
        all_services.first().is_some_and(|svc| {
            let branch =
                crate::cmd::worktree::current_branch(&root.join(svc)).unwrap_or_default();
            extract_ticket(&tm.pattern, &branch).is_some()
        })
    };

    let notes = SessionNotes::new()
        .add("reload_env", "reload .env from current preset")
        .add("change_preset", "switch preset")
        .add("close_session", "close session")
        .add("show_info", "re-display this box")
        .add("inspect_env", "inspect env file ages")
        .add_if(has_ticket, "open_ticket", "open ticket in browser")
        .add("open_pr", "open GitHub PR in browser");

    let show_info_fn_cmd = format!(
        "PENV_REPO_ROOT='{}' '{}' show-info --preset '{}' --services {}{} --notes {} ||:",
        sh_escape(&repo_root().to_string_lossy()),
        sh_escape(&penv),
        sh_escape(&preset),
        services_arg,
        message_arg,
        notes.to_show_info_args(),
    );
    crate::cmd::worktree::write_close_session_script_with_reload(
        &helper_path,
        session,
        &penv,
        &preset,
        &svc_refs,
        &show_info_fn_cmd,
        reload_cmd_override,
    )?;
    mux.send_keys(
        &helper_pane,
        &format!(
            "{}; source '{}'; rm -f '{}'",
            show_info_fn_cmd, helper_path, helper_path,
        ),
    )?;

    // ── Claude window + attach ────────────────────────────────────────────────

    setup_linked_window(session, &settings.project.claude, mux.as_ref())?;

    mux.select_window(session, 0)?;
    mux.attach(session);
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
            title: None,
        }
    }

    #[test]
    fn grid_dims_single_pane() {
        let tab = TabConfig {
            name: "main".to_string(),
            panes: vec![pane("my-api", "npm run dev", 0, 0)],
        };
        let n_rows = tab.panes.iter().map(|p| p.row + 1).max().unwrap_or(1);
        let n_cols = tab
            .panes
            .iter()
            .map(|p| p.col + 1)
            .max()
            .unwrap_or(1)
            .min(2);
        assert_eq!((n_rows, n_cols), (1, 1));
    }

    #[test]
    fn grid_dims_2x2() {
        let tab = TabConfig {
            name: "main".to_string(),
            panes: vec![
                pane("my-api", "npm run dev", 0, 0),
                pane("my-ui", "npm run dev", 0, 1),
                pane("my-ui", "npm run sb", 1, 1),
            ],
        };
        let n_rows = tab.panes.iter().map(|p| p.row + 1).max().unwrap_or(1);
        let n_cols = tab
            .panes
            .iter()
            .map(|p| p.col + 1)
            .max()
            .unwrap_or(1)
            .min(2);
        assert_eq!((n_rows, n_cols), (2, 2));
    }

    #[test]
    fn grid_dims_col_capped_at_2() {
        let tab = TabConfig {
            name: "main".to_string(),
            panes: vec![pane("svc", "cmd", 5, 0)],
        };
        let n_cols = tab
            .panes
            .iter()
            .map(|p| p.col + 1)
            .max()
            .unwrap_or(1)
            .min(2);
        assert_eq!(n_cols, 2);
    }

    #[test]
    fn helper_pane_is_first_unconfigured_position() {
        let tab = TabConfig {
            name: "main".to_string(),
            panes: vec![
                pane("my-api", "npm run dev", 0, 0),
                // (0,1) and (1,0) unconfigured → (0,1) wins (row=0 first)
                pane("my-ui", "npm run sb", 1, 1),
            ],
        };
        let n_rows = tab.panes.iter().map(|p| p.row + 1).max().unwrap_or(1);
        let n_cols = tab
            .panes
            .iter()
            .map(|p| p.col + 1)
            .max()
            .unwrap_or(1)
            .min(2);
        assert_eq!((n_rows, n_cols), (2, 2));

        let mut helper: Option<(usize, usize)> = None;
        'outer: for r in 0..n_rows {
            for c in 0..n_cols {
                let has_cmd = tab
                    .panes
                    .iter()
                    .any(|p| p.row == r && p.col == c && !p.cmd.is_empty());
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
            panes: vec![pane("a", "cmd1", 0, 0), pane("b", "cmd2", 1, 0)],
        };
        let n_rows = tab.panes.iter().map(|p| p.row + 1).max().unwrap_or(1);
        let n_cols = tab
            .panes
            .iter()
            .map(|p| p.col + 1)
            .max()
            .unwrap_or(1)
            .min(2);

        let mut helper: Option<(usize, usize)> = None;
        'outer: for r in 0..n_rows {
            for c in 0..n_cols {
                let has_cmd = tab
                    .panes
                    .iter()
                    .any(|p| p.row == r && p.col == c && !p.cmd.is_empty());
                if !has_cmd {
                    helper = Some((r, c));
                    break 'outer;
                }
            }
        }
        assert_eq!(helper, None);
    }
}
