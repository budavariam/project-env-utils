//! `penv dev-ui [--preset <name>] [--select] [--resume [<branch>]]`
//!
//! Launches the ui repo in a tmux session.
use anyhow::{bail, Result};

use crate::cmd::pick_preset::resolve_workspace_preset;
use crate::cmd::worktree::{
    branch_to_safe, ensure_worktree, pick_branch_fzf, pick_worktree_wtf, write_close_session_script,
    write_teardown_script,
};
use crate::config::{repo_parent, repo_root, Settings};
use crate::tmux::{
    active_mux, list_panes, setup_linked_window, tmux, tmux_send_keys,
};

// ── Public types ───────────────────────────────────────────────────────────────

pub struct DevUiArgs {
    pub preset: Option<String>,
    pub select: bool,
    pub resume: bool,
    pub resume_branch: Option<String>,
    pub attach: bool,
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

    let (ui_dir, session, is_worktree) = if args.select {
        let (wt_path, branch) = pick_worktree_wtf(&ui_dir)?;
        let session = format!("{}_{}", repo, branch_to_safe(&branch));
        eprintln!("Session: {}", session);
        (wt_path, session, true)
    } else if args.resume {
        let branch = if let Some(b) = &args.resume_branch {
            b.clone()
        } else {
            pick_branch_fzf(&ui_dir)?
        };
        let wt = ensure_worktree(&ui_dir, &branch)?;
        let session = format!("{}_{}", repo, branch_to_safe(&branch));
        eprintln!(
            "Workspace: {} (branch: {}, session: {})",
            wt.display(),
            branch,
            session
        );
        (wt, session, true)
    } else {
        (ui_dir.clone(), repo.clone(), false)
    };

    // ── Build backend ────────────────────────────────────────────────────────

    let backend_box = crate::backend::active_backend(settings);
    let bref = backend_box.as_deref();

    // ── Resolve preset ───────────────────────────────────────────────────────

    let ui_dir_str = ui_dir.to_string_lossy().into_owned();
    let preset = resolve_workspace_preset(repo, &ui_dir_str, args.preset.as_deref(), bref, settings)?;

    // ── Session guard ────────────────────────────────────────────────────────

    crate::cmd::morning_check::startup_sync_check(
        &[(repo.as_str(), Some(ui_dir_str.as_str()))],
        &preset,
        bref,
        settings,
    );
    println!();

    if mux.session_exists(&session) {
        eprintln!("Session '{}' already exists. Attaching...", session);
        mux.attach(&session);
    }

    // ── Create session and first window "ui" ─────────────────────────────────

    let ui_dir_s = ui_dir.to_string_lossy();
    tmux(&["new-session", "-d", "-s", &session, "-n", "ui", "-c", &ui_dir_s])?;

    tmux(&["split-window", "-h", "-l", "50%", "-c", &ui_dir_s])?;

    let panes_after_h = list_panes(&session)?;
    if panes_after_h.len() < 2 {
        bail!("pane ID resolution failed after h-split");
    }
    let p_ui = panes_after_h[0].0.clone();
    let p_shell = panes_after_h[1].0.clone();

    tmux(&[
        "split-window", "-v", "-l", "50%", "-t", &p_ui, "-c", &ui_dir_s,
    ])?;

    let panes_after_v = list_panes(&session)?;
    if panes_after_v.len() < 3 {
        bail!("pane ID resolution failed after v-split");
    }
    let p_sb = panes_after_v[1].0.clone();

    // ── Pane commands ─────────────────────────────────────────────────────────

    let dev_cmd = &settings.project.dev_ui.pane_dev_cmd;
    let sb_cmd = &settings.project.dev_ui.pane_sb_cmd;

    tmux_send_keys(
        &p_ui,
        &format!("cd '{}' && {}", ui_dir_s, dev_cmd),
    )?;

    tmux_send_keys(
        &p_sb,
        &format!("cd '{}' && sleep 10 && {}", ui_dir_s, sb_cmd),
    )?;

    // Shell pane
    let penv = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("{}/penv", repo_root().to_string_lossy()));
    let helper_path = format!("/tmp/dev-ui-helper-{}", session);

    if is_worktree {
        let show_info_fn_cmd = format!(
            "'{}' show-info --preset '{}' --workspace '{}' --notes 'teardown      — remove worktree & close session' 'reload_env    — reload .env from current preset' 'close_session — close session only' 'show_info     — re-display this box' 'inspect_env   — inspect env file ages' ||:",
            penv, preset, ui_dir_s,
        );
        write_teardown_script(&helper_path, &session, &[(&ui_dir, &root.join(repo))], &penv, &preset, &[repo.as_str()], &show_info_fn_cmd)?;
        tmux_send_keys(
            &p_shell,
            &format!(
                "{}; source '{}'; rm -f '{}'",
                show_info_fn_cmd, helper_path, helper_path,
            ),
        )?;
    } else {
        let show_info_fn_cmd = format!(
            "'{}' show-info --preset '{}' --workspace '{}' --notes 'reload_env    — reload .env from current preset' 'close_session — close session' 'show_info     — re-display this box' 'inspect_env   — inspect env file ages' ||:",
            penv, preset, ui_dir_s,
        );
        write_close_session_script(&helper_path, &session, &penv, &preset, &[repo.as_str()], &show_info_fn_cmd)?;
        tmux_send_keys(
            &p_shell,
            &format!(
                "{}; source '{}'; rm -f '{}'",
                show_info_fn_cmd, helper_path, helper_path,
            ),
        )?;
    }

    // ── Claude window ─────────────────────────────────────────────────────────

    setup_linked_window(&session, &settings.project.claude)?;

    // ── Focus and attach ──────────────────────────────────────────────────────

    tmux(&["select-window", "-t", &format!("{}:ui", session)])?;
    tmux(&["select-pane", "-t", &p_shell])?;
    mux.attach(&session);
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
        write_close_session_script(&p, "my-ui", "/usr/local/bin/penv", "test", &["my-ui"], "penv show-info").unwrap();
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
        write_teardown_script(&p, "my-ui_feat_foo", &[(&wt, &repo)], "/usr/local/bin/penv", "test", &["my-ui"], "penv show-info").unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.contains("teardown"));
        assert!(s.contains("close_session"));
        assert!(s.contains("reload_env"));
        assert!(s.contains("show_info"));
    }
}
