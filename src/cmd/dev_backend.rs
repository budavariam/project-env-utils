//! `penv dev-backend [--preset <name>] [--select] [--resume [<branch>]]`
//!
//! Launches backend services in a tmux session.
//! window_backend repos support worktrees; window_service repos always run from main.
use std::path::PathBuf;

use anyhow::{bail, Result};

use crate::cmd::pick_preset::{resolve_backend_preset, resolve_workspace_preset};
use crate::cmd::worktree::{
    branch_to_safe, ensure_worktree, pick_branch_fzf, pick_existing_worktree_fzf,
    write_close_session_script, write_teardown_script,
};
use crate::config::{repo_parent, repo_root, Settings};
use crate::tmux::{
    active_mux, first_pane_id, setup_linked_window, tmux, tmux_output, tmux_send_keys,
};

// ── Public types ───────────────────────────────────────────────────────────────

pub struct DevBackendArgs {
    pub preset: Option<String>,
    pub select: bool,
    pub resume: bool,
    pub resume_branch: Option<String>,
    pub attach: bool,
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

    // ── Resolve worktrees (when --select / --resume) ─────────────────────────

    let (backend_dirs, session, is_worktree) = if args.select || args.resume {
        // Pick branch from the first backend repo
        let primary_repo = root.join(&cfg.window_backend[0].repo);
        let branch = if args.resume {
            args.resume_branch
                .clone()
                .map(Ok)
                .unwrap_or_else(|| pick_existing_worktree_fzf(&primary_repo))?
        } else {
            // --select: pick any branch and create a new worktree
            pick_branch_fzf(&primary_repo)?
        };

        let safe = branch_to_safe(&branch);
        let session = format!("{}_{}", cfg.session_name, safe);

        let mut dirs = Vec::new();
        for pane in &cfg.window_backend {
            let repo_dir = root.join(&pane.repo);
            let wt = ensure_worktree(&repo_dir, &branch)?;
            eprintln!(
                "  {} → {} (branch: {})",
                pane.repo,
                wt.display(),
                branch
            );
            dirs.push(wt);
        }

        (dirs, session, true)
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
        // Load .env into each worktree dir individually
        let mut preset_name = String::new();
        for (i, pane) in cfg.window_backend.iter().enumerate() {
            let ws = backend_dirs[i].to_string_lossy().into_owned();
            let p = resolve_workspace_preset(&pane.repo, &ws, args.preset.as_deref(), bref, settings)?;
            if preset_name.is_empty() {
                preset_name = p;
            }
        }
        preset_name
    } else {
        resolve_backend_preset(args.preset.as_deref(), bref, settings)?
    };

    // ── Session guard ─────────────────────────────────────────────────────────

    let sync_items: Vec<(&str, Option<&str>)> =
        cfg.window_backend.iter().map(|p| (p.repo.as_str(), None)).collect();
    crate::cmd::morning_check::startup_sync_check(&sync_items, &preset, bref, settings);
    println!();

    if mux.session_exists(&session) {
        eprintln!("Session '{}' already exists. Attaching...", session);
        mux.attach(&session);
    }

    let root_s = root.to_string_lossy();
    let penv = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("{}/penv", repo_root().to_string_lossy()));

    // ── Window 0: backend (window_backend panes) ──────────────────────────────

    let first_dir = backend_dirs[0].to_string_lossy().into_owned();
    tmux(&[
        "new-session", "-d", "-s", &session, "-n", "backend", "-c", &first_dir,
    ])?;

    let p_first = first_pane_id(&session, 0)?;

    // Shell pane (right side, horizontal split from first pane)
    let p_shell = tmux_output(&[
        "split-window", "-h", "-l", "50%", "-t", &p_first, "-c", &root_s,
        "-P", "-F", "#{pane_id}",
    ])?;
    if p_shell.is_empty() {
        bail!("pane ID resolution failed for shell pane");
    }

    // Additional backend panes (vertical splits below first pane, left column)
    let mut backend_pane_ids = vec![p_first.clone()];
    for dir in backend_dirs.iter().skip(1) {
        let dir_s = dir.to_string_lossy();
        let p = tmux_output(&[
            "split-window", "-v", "-l", "50%", "-t", &p_first, "-c", &dir_s,
            "-P", "-F", "#{pane_id}",
        ])?;
        if p.is_empty() {
            bail!("pane ID resolution failed for backend window");
        }
        backend_pane_ids.push(p);
    }

    // Send commands to backend panes
    for (i, pane_cfg) in cfg.window_backend.iter().enumerate() {
        let dir_s = backend_dirs[i].to_string_lossy();
        tmux_send_keys(
            &backend_pane_ids[i],
            &format!("cd '{}' && {}", dir_s, pane_cfg.cmd),
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

    if is_worktree {
        let worktrees: Vec<(PathBuf, PathBuf)> = cfg
            .window_backend
            .iter()
            .enumerate()
            .map(|(i, pane)| (backend_dirs[i].clone(), root.join(&pane.repo)))
            .collect();
        let wt_refs: Vec<(&std::path::Path, &std::path::Path)> = worktrees
            .iter()
            .map(|(wt, repo)| (wt.as_path(), repo.as_path()))
            .collect();
        let svc_refs: Vec<&str> = service_names.iter().map(|s| s.as_str()).collect();
        let show_info_fn_cmd = format!(
            "'{}' show-info --preset '{}' --services {} --notes 'teardown      — remove worktrees & close session' 'reload_env    — reload .env from current preset' 'close_session — close session only' 'show_info     — re-display this box' 'inspect_env   — inspect env file ages' ||:",
            penv, preset, services_arg,
        );
        write_teardown_script(&helper_path, &session, &wt_refs, &penv, &preset, &svc_refs, &show_info_fn_cmd)?;
        tmux_send_keys(
            &p_shell,
            &format!(
                "{}; source '{}'; rm -f '{}'",
                show_info_fn_cmd, helper_path, helper_path,
            ),
        )?;
    } else {
        let svc_refs: Vec<&str> = service_names.iter().map(|s| s.as_str()).collect();
        let show_info_fn_cmd = format!(
            "'{}' show-info --preset '{}' --services {} --notes 'reload_env    — reload .env from current preset' 'close_session — close session' 'show_info     — re-display this box' 'inspect_env   — inspect env file ages' ||:",
            penv, preset, services_arg,
        );
        write_close_session_script(&helper_path, &session, &penv, &preset, &svc_refs, &show_info_fn_cmd)?;
        tmux_send_keys(
            &p_shell,
            &format!(
                "{}; source '{}'; rm -f '{}'",
                show_info_fn_cmd, helper_path, helper_path,
            ),
        )?;
    }

    // ── Window 1: service (window_service panes) ──────────────────────────────

    if !cfg.window_service.is_empty() {
        let first_svc_dir = root.join(&cfg.window_service[0].repo);
        let first_svc_s = first_svc_dir.to_string_lossy();
        tmux(&["new-window", "-t", &session, "-n", "service", "-c", &first_svc_s])?;

        let p_svc_first = first_pane_id(&session, 1)?;

        let mut svc_pane_ids = vec![p_svc_first.clone()];
        for pane_cfg in cfg.window_service.iter().skip(1) {
            let dir_s = root.join(&pane_cfg.repo).to_string_lossy().into_owned();
            let p = tmux_output(&[
                "split-window", "-h", "-l", "50%", "-t", &p_svc_first,
                "-c", &dir_s, "-P", "-F", "#{pane_id}",
            ])?;
            if p.is_empty() {
                bail!("pane ID resolution failed for service window");
            }
            svc_pane_ids.push(p);
        }

        for (i, pane_cfg) in cfg.window_service.iter().enumerate() {
            let dir_s = root.join(&pane_cfg.repo).to_string_lossy().into_owned();
            tmux_send_keys(
                &svc_pane_ids[i],
                &format!("cd '{}' && {}", dir_s, pane_cfg.cmd),
            )?;
        }
    }

    // ── Claude window ─────────────────────────────────────────────────────────

    setup_linked_window(&session, &settings.project.claude)?;

    // ── Focus and attach ──────────────────────────────────────────────────────

    tmux(&["select-window", "-t", &format!("{}:0", session)])?;
    mux.attach(&session);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_session_script_references_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper.sh").to_string_lossy().into_owned();
        write_close_session_script(&path, "my-backend", "/usr/local/bin/penv", "test", &["my-api"], "penv show-info").unwrap();
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
        write_teardown_script(&path, "my-backend_feat_x", &[
            (wt1.as_path(), repo1.as_path()),
            (wt2.as_path(), repo2.as_path()),
        ], "/usr/local/bin/penv", "test", &["svc-a", "svc-b"], "penv show-info").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("teardown"));
        assert!(content.contains("close_session"));
        assert!(content.contains("reload_env"));
        assert!(content.contains("show_info"));
        assert!(content.contains("svc-a"));
        assert!(content.contains("svc-b"));
    }
}
