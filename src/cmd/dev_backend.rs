//! `penv dev-backend [--preset <name>] [--checkout [<branch>]] [--worktree] [--checkout-worktree [<branch>]]`
//!
//! Launches backend services in a dev session using the configured multiplexer.
//! window_backend repos support worktrees; window_service repos always run from main.
use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::cmd::open_ticket::extract_ticket;
use crate::cmd::pick_preset::{resolve_backend_preset, resolve_workspace_preset};
use crate::cmd::show_info::SessionNotes;
use crate::cmd::worktree::{
    branch_to_safe, current_branch, ensure_worktree, pick_branch_fzf, pick_existing_worktree_fzf,
    stash_and_checkout, write_close_session_script, write_teardown_script,
};
use crate::config::{Settings, repo_parent, repo_root};
use crate::env_file::sh_escape;
use crate::tmux::{active_mux, setup_linked_window};

// ── Public types ───────────────────────────────────────────────────────────────

pub struct DevBackendArgs {
    pub preset: Option<String>,
    /// Open the session in an existing Claude worktree (fzf picker).
    pub worktree: bool,
    /// Checkout a branch directly in the root repos (stash changes first).
    pub checkout: bool,
    pub checkout_branch: Option<String>,
    /// Checkout a branch into a new/existing Claude worktree.
    pub checkout_worktree: bool,
    pub checkout_worktree_branch: Option<String>,
    pub attach: bool,
    /// Pre-selected worktree branch; bypasses the interactive picker when set with `worktree: true`.
    pub worktree_branch: Option<String>,
}

// ── run() ─────────────────────────────────────────────────────────────────────

pub fn run(args: &DevBackendArgs, settings: &Settings) -> Result<()> {
    let mux = active_mux(settings);
    let root = repo_parent();
    let cfg = &settings.project.dev_backend;

    if cfg.window_backend.is_empty() {
        bail!("dev_backend.window_backend is empty in settings.json — nothing to launch");
    }

    // ── --attach: reconnect to an existing session ────────────────────────────

    if args.attach {
        let prefix = &cfg.session_name;
        let mut sessions = mux.sessions_with_prefix(prefix);
        sessions.sort();
        let target = match sessions.len() {
            0 => bail!("No running session found with prefix '{}'", prefix),
            1 => sessions.remove(0),
            _ => crate::cmd::worktree::pick_session_fzf(&sessions)?,
        };
        eprintln!("Attaching to '{}'...", target);
        mux.attach(&target);
    }

    // ── Resolve workspace ────────────────────────────────────────────────────

    let (backend_dirs, session, is_worktree) = if args.worktree {
        // --worktree: pick from existing Claude worktrees in the primary repo,
        // or use the pre-selected branch passed by the caller.
        let primary_repo = root.join(&cfg.window_backend[0].repo);
        let branch = match args.worktree_branch.clone() {
            Some(b) => b,
            None => pick_existing_worktree_fzf(&primary_repo)?,
        };
        let safe = branch_to_safe(&branch);
        let session = format!("{}_{}", cfg.session_name, safe);
        let mut dirs = Vec::new();
        for pane in &cfg.window_backend {
            let repo_dir = root.join(&pane.repo);
            let wt = ensure_worktree(&repo_dir, &branch)?;
            eprintln!("  {} → {} (branch: {})", pane.repo, wt.display(), branch);
            dirs.push(wt);
        }
        (dirs, session, true)
    } else if args.checkout_worktree {
        // --checkout-worktree: create/reuse a Claude worktree for the branch.
        // Error if the branch is currently checked out in the root repo.
        let primary_repo = root.join(&cfg.window_backend[0].repo);
        let branch = args
            .checkout_worktree_branch
            .clone()
            .map(Ok)
            .unwrap_or_else(|| pick_branch_fzf(&primary_repo))?;

        let root_branch = current_branch(&primary_repo).unwrap_or_default();
        if root_branch == branch {
            bail!(
                "Branch '{}' is currently checked out in the root repo. \
                 Use --checkout to open the session there instead.",
                branch
            );
        }

        let safe = branch_to_safe(&branch);
        let session = format!("{}_{}", cfg.session_name, safe);
        let mut dirs = Vec::new();
        for pane in &cfg.window_backend {
            let repo_dir = root.join(&pane.repo);
            let wt = ensure_worktree(&repo_dir, &branch)?;
            eprintln!("  {} → {} (branch: {})", pane.repo, wt.display(), branch);
            dirs.push(wt);
        }
        (dirs, session, true)
    } else if args.checkout {
        // --checkout: stash changes if needed and checkout branch in the root repos.
        let primary_repo = root.join(&cfg.window_backend[0].repo);
        let branch = args
            .checkout_branch
            .clone()
            .map(Ok)
            .unwrap_or_else(|| pick_branch_fzf(&primary_repo))?;

        eprintln!("Checking out '{}' in backend repos...", branch);
        for pane in &cfg.window_backend {
            let repo_dir = root.join(&pane.repo);
            stash_and_checkout(&repo_dir, &branch)?;
        }

        let dirs: Vec<PathBuf> = cfg
            .window_backend
            .iter()
            .map(|p| root.join(&p.repo))
            .collect();
        (dirs, cfg.session_name.clone(), false)
    } else {
        let dirs: Vec<PathBuf> = cfg
            .window_backend
            .iter()
            .map(|p| root.join(&p.repo))
            .collect();
        (dirs, cfg.session_name.clone(), false)
    };

    // ── Build backend and resolve preset ─────────────────────────────────────

    let backend_box = crate::backend::active_backend(settings);
    let bref = backend_box.as_deref();

    let preset = if is_worktree {
        let mut preset_name = String::new();
        for (i, pane) in cfg.window_backend.iter().enumerate() {
            let ws = backend_dirs[i].to_string_lossy().into_owned();
            let p =
                resolve_workspace_preset(&pane.repo, &ws, args.preset.as_deref(), bref, settings)?;
            if preset_name.is_empty() {
                preset_name = p;
            }
        }
        preset_name
    } else {
        resolve_backend_preset(args.preset.as_deref(), bref, settings)?
    };

    // ── Session guard ─────────────────────────────────────────────────────────

    let sync_items: Vec<(&str, Option<&str>)> = cfg
        .window_backend
        .iter()
        .map(|p| (p.repo.as_str(), None))
        .collect();
    crate::cmd::morning_check::startup_sync_check(&sync_items, &preset, bref, settings);
    println!();

    if mux.session_exists(&session) {
        eprintln!("Session '{}' already exists. Attaching...", session);
        mux.attach(&session);
    }

    let root_s = root.to_string_lossy().into_owned();
    let penv = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("{}/penv", repo_root().to_string_lossy()));

    // ── Window 0: backend (window_backend panes) ──────────────────────────────
    //
    // Layout:  backend_pane_0  | shell_pane
    //          backend_pane_1  |
    //          ...

    let first_dir = backend_dirs[0].to_string_lossy().into_owned();
    mux.new_session(&session, "backend", &first_dir)?;
    let p_first = mux.first_pane_id(&session, 0)?;

    // Shell pane: horizontal split to the right
    let p_shell = mux.split_h(&p_first, &root_s)?;

    // Additional backend panes: vertical splits below the first
    let mut backend_pane_ids = vec![p_first.clone()];
    for dir in backend_dirs.iter().skip(1) {
        let dir_s = dir.to_string_lossy().into_owned();
        let p = mux.split_v(&p_first, &dir_s)?;
        if p.is_empty() {
            bail!("pane ID resolution failed for backend pane");
        }
        backend_pane_ids.push(p);
    }

    // Send commands to backend panes
    for (i, pane_cfg) in cfg.window_backend.iter().enumerate() {
        let dir_s = backend_dirs[i].to_string_lossy().into_owned();
        mux.send_keys(
            &backend_pane_ids[i],
            &format!("cd '{}' && {}", sh_escape(&dir_s), pane_cfg.cmd),
        )?;
    }

    // Shell pane — show info + helper script
    let helper_path = format!("/tmp/dev-backend-helper-{}", session);
    let service_names: Vec<String> = cfg.window_backend.iter().map(|p| p.repo.clone()).collect();
    let services_arg = service_names
        .iter()
        .map(|s| format!("'{}'", s))
        .collect::<Vec<_>>()
        .join(" ");

    let branch_name = if is_worktree {
        backend_dirs
            .first()
            .and_then(|d| d.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string()
    } else {
        cfg.window_backend
            .first()
            .map(|p| current_branch(&root.join(&p.repo)).unwrap_or_default())
            .unwrap_or_default()
    };
    let tm = &settings.project.ticket_manager;
    let has_ticket = tm.is_configured() && extract_ticket(&tm.pattern, &branch_name).is_some();

    let notes = SessionNotes::new()
        .add_if(is_worktree, "teardown", "remove worktrees & close session")
        .add("reload_env", "reload .env from current preset")
        .add("change_preset", "switch preset")
        .add_if(is_worktree, "close_session", "close session only")
        .add_if(!is_worktree, "close_session", "close session")
        .add("show_info", "re-display this box")
        .add("inspect_env", "inspect env file ages")
        .add_if(has_ticket, "open_ticket", "open ticket in browser")
        .add("open_pr", "open GitHub PR in browser");

    let show_info_fn_cmd = format!(
        "'{}' show-info --preset '{}' --services {} --notes {} ||:",
        sh_escape(&penv),
        sh_escape(&preset),
        services_arg,
        notes.to_show_info_args(),
    );

    if is_worktree {
        let worktrees: Vec<(PathBuf, PathBuf)> = cfg
            .window_backend
            .iter()
            .enumerate()
            .map(|(i, pane)| (backend_dirs[i].clone(), root.join(&pane.repo)))
            .collect();
        let wt_refs: Vec<(&std::path::Path, &std::path::Path)> = worktrees
            .iter()
            .map(|(wt, r)| (wt.as_path(), r.as_path()))
            .collect();
        let svc_refs: Vec<&str> = service_names.iter().map(|s| s.as_str()).collect();
        write_teardown_script(
            &helper_path,
            &session,
            &wt_refs,
            &penv,
            &preset,
            &svc_refs,
            &show_info_fn_cmd,
        )?;
    } else {
        let svc_refs: Vec<&str> = service_names.iter().map(|s| s.as_str()).collect();
        write_close_session_script(
            &helper_path,
            &session,
            &penv,
            &preset,
            &svc_refs,
            &show_info_fn_cmd,
        )?;
    }
    mux.send_keys(
        &p_shell,
        &format!(
            "{}; source '{}'; rm -f '{}'",
            show_info_fn_cmd, helper_path, helper_path
        ),
    )?;

    // ── Window 1: service (window_service panes) ──────────────────────────────

    if !cfg.window_service.is_empty() {
        let first_svc_s = root
            .join(&cfg.window_service[0].repo)
            .to_string_lossy()
            .into_owned();
        mux.new_window(&session, "service", &first_svc_s)?;
        let p_svc_first = mux.first_pane_id(&session, 1)?;

        let mut svc_pane_ids = vec![p_svc_first.clone()];
        for pane_cfg in cfg.window_service.iter().skip(1) {
            let dir_s = root.join(&pane_cfg.repo).to_string_lossy().into_owned();
            let p = mux.split_h(&p_svc_first, &dir_s)?;
            if p.is_empty() {
                bail!("pane ID resolution failed for service window");
            }
            svc_pane_ids.push(p);
        }

        for (i, pane_cfg) in cfg.window_service.iter().enumerate() {
            let dir_s = root.join(&pane_cfg.repo).to_string_lossy().into_owned();
            mux.send_keys(
                &svc_pane_ids[i],
                &format!("cd '{}' && {}", sh_escape(&dir_s), pane_cfg.cmd),
            )?;
        }
    }

    // ── Claude window ─────────────────────────────────────────────────────────

    setup_linked_window(&session, &settings.project.claude, mux.as_ref())?;

    // ── Focus and attach ──────────────────────────────────────────────────────

    mux.select_window(&session, 0)?;
    mux.attach(&session);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_session_script_references_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper.sh").to_string_lossy().into_owned();
        write_close_session_script(
            &path,
            "my-backend",
            "/usr/local/bin/penv",
            "test",
            &["my-api"],
            "penv show-info",
        )
        .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("close_session"));
        assert!(content.contains("reload_env"));
        assert!(content.contains("show_info"));
        assert!(content.contains("my-backend"));
        assert!(!content.contains("teardown"));
    }

    #[test]
    fn worktree_teardown_covers_all_repos() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper.sh").to_string_lossy().into_owned();
        let wt1 = PathBuf::from("/repos/svc-a/.claude/worktrees/feat_x");
        let repo1 = PathBuf::from("/repos/svc-a");
        let wt2 = PathBuf::from("/repos/svc-b/.claude/worktrees/feat_x");
        let repo2 = PathBuf::from("/repos/svc-b");
        write_teardown_script(
            &path,
            "my-backend_feat_x",
            &[
                (wt1.as_path(), repo1.as_path()),
                (wt2.as_path(), repo2.as_path()),
            ],
            "/usr/local/bin/penv",
            "test",
            &["svc-a", "svc-b"],
            "penv show-info",
        )
        .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("teardown"));
        assert!(content.contains("close_session"));
        assert!(content.contains("reload_env"));
        assert!(content.contains("show_info"));
        assert!(content.contains("svc-a"));
        assert!(content.contains("svc-b"));
    }
}
