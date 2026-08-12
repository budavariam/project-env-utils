//! `penv dev-ui [--preset <name>] [--checkout [<branch>]] [--worktree] [--checkout-worktree [<branch>]]`
//!
//! Launches the ui repo in a dev session using the configured multiplexer.
use anyhow::{Result, bail};

use crate::cmd::open_ticket::extract_ticket;
use crate::cmd::pick_preset::resolve_workspace_preset;
use crate::cmd::show_info::SessionNotes;
use crate::cmd::worktree::{
    branch_to_safe, current_branch, ensure_worktree, pick_branch_fzf, pick_worktree_wtf,
    stash_and_checkout, write_close_session_script, write_teardown_script,
};
use crate::config::{Settings, repo_parent, repo_root};
use crate::env_file::sh_escape;
use crate::tmux::{active_mux, setup_linked_window};

// ── Public types ───────────────────────────────────────────────────────────────

pub struct DevUiArgs {
    pub preset: Option<String>,
    /// Open the session in an existing Claude worktree (fzf picker).
    pub worktree: bool,
    /// Checkout a branch directly in the root repo (stash changes first).
    pub checkout: bool,
    pub checkout_branch: Option<String>,
    /// Checkout a branch into a new/existing Claude worktree.
    pub checkout_worktree: bool,
    pub checkout_worktree_branch: Option<String>,
    pub attach: bool,
    /// Pre-selected worktree path; bypasses the interactive picker when set.
    /// Callers that need to know the UI branch before `run()` (e.g. to open VS Code
    /// with the right group arg) can resolve the worktree themselves and pass it here.
    pub worktree_path: Option<std::path::PathBuf>,
    /// Skip the final tmux attach (used when a caller starts multiple sessions and attaches separately).
    pub no_attach: bool,
}

// ── run() ─────────────────────────────────────────────────────────────────────

pub fn run(args: &DevUiArgs, settings: &Settings) -> Result<()> {
    let mux = active_mux(settings);
    let root = repo_parent();
    let repo = &settings.project.dev_ui.repo;
    let ui_dir = root.join(repo);

    // ── --attach: reconnect to an existing session ────────────────────────────

    if args.attach {
        let mut sessions = mux.sessions_with_prefix(repo);
        sessions.sort();
        let target = match sessions.len() {
            0 => bail!("No running session found with prefix '{}'", repo),
            1 => sessions.remove(0),
            _ => crate::cmd::worktree::pick_session_fzf(&sessions)?,
        };
        eprintln!("Attaching to '{}'...", target);
        mux.attach(&target);
    }

    // ── Resolve workspace ────────────────────────────────────────────────────

    let (ui_dir, session, is_worktree) = if let Some(wt_path) = args.worktree_path.clone() {
        // Caller pre-selected the worktree (e.g. to thread the branch into a VS Code open).
        let branch = wt_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let session = format!("{}_{}", repo, branch_to_safe(&branch));
        eprintln!("Session: {}", session);
        (wt_path, session, true)
    } else if args.worktree {
        // --worktree: pick from existing Claude worktrees.
        let (wt_path, branch) = pick_worktree_wtf(&ui_dir)?;
        let session = format!("{}_{}", repo, branch_to_safe(&branch));
        eprintln!("Session: {}", session);
        (wt_path, session, true)
    } else if args.checkout_worktree {
        // --checkout-worktree: create/reuse a Claude worktree for the branch.
        // Error if that branch is currently checked out in the root repo.
        let branch = args
            .checkout_worktree_branch
            .clone()
            .map(Ok)
            .unwrap_or_else(|| pick_branch_fzf(&ui_dir))?;

        let root_branch = current_branch(&ui_dir).unwrap_or_default();
        if root_branch == branch {
            bail!(
                "Branch '{}' is currently checked out in the root repo. \
                 Use --checkout to open the session there instead.",
                branch
            );
        }

        let wt = ensure_worktree(&ui_dir, &branch)?;
        let session = format!("{}_{}", repo, branch_to_safe(&branch));
        eprintln!(
            "Workspace: {} (branch: {}, session: {})",
            wt.display(),
            branch,
            session
        );
        (wt, session, true)
    } else if args.checkout || args.checkout_branch.is_some() {
        // --checkout / --branch <name>: stash changes and checkout branch in the root repo.
        let branch = args
            .checkout_branch
            .clone()
            .map(Ok)
            .unwrap_or_else(|| pick_branch_fzf(&ui_dir))?;

        eprintln!("Checking out '{}' in {}...", branch, repo);
        stash_and_checkout(&ui_dir, &branch)?;
        (ui_dir.clone(), repo.clone(), false)
    } else {
        (ui_dir.clone(), repo.clone(), false)
    };

    // ── Build backend ────────────────────────────────────────────────────────

    let backend_box = crate::backend::active_backend(settings);
    let bref = backend_box.as_deref();

    // ── Resolve preset ───────────────────────────────────────────────────────

    let ui_dir_s = ui_dir.to_string_lossy().into_owned();
    let preset = resolve_workspace_preset(repo, &ui_dir_s, args.preset.as_deref(), bref, settings)?;

    // ── Session guard ────────────────────────────────────────────────────────

    crate::cmd::morning_check::startup_sync_check(
        &[(repo.as_str(), Some(ui_dir_s.as_str()))],
        &preset,
        bref,
        settings,
    );
    println!();

    if mux.session_exists(&session) {
        eprintln!("Session '{}' already exists. Attaching...", session);
        if !args.no_attach {
            mux.attach(&session);
        }
        return Ok(());
    }

    // ── Create session with 2×2 pane grid ────────────────────────────────────
    //
    // (col=1, row=0) is always the shell/info pane; all other positions come from
    // dev_ui.panes in settings.json — empty cmd = idle shell.
    //
    // Layout (tmux):               Layout (screen — each cell = a window):
    //   [0][0]  | [0][1]=shell       window 0 = [0][0]
    //   [1][0]  | [1][1]             window p1 = shell
    //                                window p2 = [1][0]
    //                                window p3 = [1][1]

    mux.new_session(&session, "ui", &ui_dir_s)?;
    let first = mux.first_pane_id(&session, 0)?;
    let grid = mux.build_pane_grid(&first, 2, 2, &ui_dir_s)?;
    let p_shell = &grid[0][1];

    // ── Pane commands (fully driven by settings.json dev_ui.panes) ────────────

    for pane_cfg in &settings.project.dev_ui.panes {
        let col = pane_cfg.col.min(1);
        let row = pane_cfg.row;
        if col == 1 && row == 0 {
            continue; // reserved for the shell pane
        }
        if row < grid.len() && col < grid[row].len() && !pane_cfg.cmd.is_empty() {
            let dir = if pane_cfg.repo.is_empty() {
                ui_dir_s.clone()
            } else {
                root.join(&pane_cfg.repo).to_string_lossy().into_owned()
            };
            mux.send_keys(
                &grid[row][col],
                &format!("cd '{}' && {}", sh_escape(&dir), pane_cfg.cmd),
            )?;
        }
    }

    // Shell pane — helper script
    let penv = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("{}/penv", repo_root().to_string_lossy()));
    let helper_path = format!("/tmp/dev-ui-helper-{}", session);

    let branch_name = if is_worktree {
        ui_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string()
    } else {
        current_branch(&ui_dir).unwrap_or_default()
    };
    let tm = &settings.project.ticket_manager;
    let has_ticket = tm.is_configured() && extract_ticket(&tm.pattern, &branch_name).is_some();

    let notes = SessionNotes::new()
        .add_if(is_worktree, "teardown", "remove worktree & close session")
        .add("reload_env", "reload .env from current preset")
        .add("change_preset", "switch preset")
        .add_if(is_worktree, "close_session", "close session only")
        .add_if(!is_worktree, "close_session", "close session")
        .add("show_info", "re-display this box")
        .add("inspect_env", "inspect env file ages")
        .add_if(has_ticket, "open_ticket", "open ticket in browser")
        .add("open_pr", "open GitHub PR in browser");

    let show_info_fn_cmd = format!(
        "'{}' show-info --preset '{}' --workspace '{}' --notes {} ||:",
        sh_escape(&penv),
        sh_escape(&preset),
        sh_escape(&ui_dir_s),
        notes.to_show_info_args(),
    );

    if is_worktree {
        write_teardown_script(
            &helper_path,
            &session,
            &[(&ui_dir, &root.join(repo))],
            &penv,
            &preset,
            &[repo.as_str()],
            &show_info_fn_cmd,
        )?;
    } else {
        write_close_session_script(
            &helper_path,
            &session,
            &penv,
            &preset,
            &[repo.as_str()],
            &show_info_fn_cmd,
        )?;
    }
    mux.send_keys(
        p_shell,
        &format!(
            "{}; source '{}'; rm -f '{}'",
            show_info_fn_cmd, helper_path, helper_path
        ),
    )?;

    // ── Claude window ─────────────────────────────────────────────────────────

    setup_linked_window(&session, &settings.project.claude, mux.as_ref())?;

    // ── Focus and attach ──────────────────────────────────────────────────────

    mux.select_window(&session, 0)?;
    mux.select_pane(p_shell)?;
    if !args.no_attach {
        mux.attach(&session);
    }
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn worktree_path_from_branch() {
        let root = PathBuf::from("/home/user/project");
        let branch = "feat/my-feature";
        let safe = branch_to_safe(branch);
        let path = root
            .join("my-ui")
            .join(".claude")
            .join("worktrees")
            .join(&safe);
        assert_eq!(
            path,
            PathBuf::from("/home/user/project/my-ui/.claude/worktrees/feat_my-feature")
        );
    }

    #[test]
    fn non_worktree_helper_has_close_session_not_teardown() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("h.sh").to_string_lossy().into_owned();
        write_close_session_script(
            &p,
            "my-ui",
            "/usr/local/bin/penv",
            "test",
            &["my-ui"],
            "penv show-info",
        )
        .unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.contains("close_session"));
        assert!(s.contains("reload_env"));
        assert!(s.contains("show_info"));
        assert!(!s.contains("teardown"));
    }

    #[test]
    fn worktree_helper_has_teardown_and_close_session() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("h.sh").to_string_lossy().into_owned();
        let wt = PathBuf::from("/repos/my-ui/.claude/worktrees/feat_foo");
        let repo = PathBuf::from("/repos/my-ui");
        write_teardown_script(
            &p,
            "my-ui_feat_foo",
            &[(&wt, &repo)],
            "/usr/local/bin/penv",
            "test",
            &["my-ui"],
            "penv show-info",
        )
        .unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.contains("teardown"));
        assert!(s.contains("close_session"));
        assert!(s.contains("reload_env"));
        assert!(s.contains("show_info"));
    }
}
