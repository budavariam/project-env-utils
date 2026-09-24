//! Interactive wrapper around `dev-ui` that asks whether to open VS Code
//! and which backend worktree to use before starting the tmux session.
use anyhow::Result;
use std::path::PathBuf;

use crate::config::{Settings, repo_parent};

use super::workspace;

pub fn run_with_vscode_prompts(
    preset: Option<String>,
    worktree: bool,
    checkout: bool,
    checkout_worktree: bool,
    branch: Option<String>,
    attach: bool,
    settings: &Settings,
) -> Result<()> {
    if attach {
        return super::dev_ui::run(
            &super::dev_ui::DevUiArgs {
                preset,
                worktree,
                checkout,
                checkout_branch: branch.clone(),
                checkout_worktree,
                checkout_worktree_branch: if checkout_worktree { branch } else { None },
                attach: true,
                worktree_path: None,
                no_attach: false,
            },
            settings,
        );
    }

    let parent = repo_parent();
    let ui_repo = &settings.project.dev_ui.repo;
    let ui_dir = parent.join(ui_repo);

    // --worktree: pick an existing worktree eagerly so run_open() gets the correct path.
    let mut pre_selected_worktree_branch: Option<String> = None;
    let pre_selected_worktree: Option<PathBuf> = if worktree {
        let (wt_path, wt_branch) = crate::cmd::worktree::pick_worktree_wtf(&ui_dir)?;
        let safe = crate::cmd::worktree::branch_to_safe(&wt_branch);
        eprintln!("Session: {}_{}", ui_repo, safe);
        pre_selected_worktree_branch = Some(safe);
        Some(wt_path)
    } else {
        None
    };

    // --checkout-worktree: resolve branch before VS Code prompts fire.
    let resolved_cw_branch: Option<String> = if checkout_worktree {
        let b = match branch.clone() {
            Some(b) => b,
            None => crate::cmd::worktree::pick_branch_fzf(&ui_dir)?,
        };
        eprintln!("  → branch: {}", b);
        Some(b)
    } else {
        None
    };

    let ui_branch_for_vscode: Option<String> = pre_selected_worktree_branch
        .clone()
        .or_else(|| resolved_cw_branch.clone());

    // ── Ask: open VS Code? ────────────────────────────────────────────────────

    let vscode_choices = vec![
        "Yes — open VS Code workspace".to_string(),
        "No — skip VS Code".to_string(),
    ];
    let vscode_sel = workspace::fzf_select(&vscode_choices, "Open VS Code? ")?;
    eprintln!("  → {}", vscode_sel);
    let open_vscode = vscode_sel.starts_with("Yes");

    if open_vscode {
        let backend_repos: Vec<String> = settings
            .project
            .dev_backend
            .window_backend
            .iter()
            .map(|p| p.repo.clone())
            .collect();

        let backend_branch: Option<String> = if !backend_repos.is_empty() {
            eprintln!("\nWhich backend?");
            let sel = workspace::pick_group_worktree(&parent, &backend_repos, "Backend> ")?;
            eprintln!("  → {}", sel.as_deref().unwrap_or("(root checkout)"));
            sel
        } else {
            None
        };

        let mut group_args: Vec<(String, String)> = Vec::new();
        if let Some(ub) = ui_branch_for_vscode.filter(|s| !s.is_empty()) {
            group_args.push(("ui".to_string(), ub));
        }
        if let Some(bb) = backend_branch.filter(|s| !s.is_empty()) {
            group_args.push(("backend".to_string(), bb));
        }

        eprintln!("\nPeacock color for this workspace?");
        let current_color = workspace::load_vscode_config()
            .ok()
            .and_then(|cfg| cfg.peacock_color);
        let peacock_color_override = workspace::pick_peacock_color(current_color.as_deref())?;

        workspace::run_open(
            settings,
            None,
            false,
            None,
            &group_args,
            peacock_color_override.as_deref(),
            false,
            false,
        )?;
    }

    // ── Start the tmux session ────────────────────────────────────────────────

    super::dev_ui::run(
        &super::dev_ui::DevUiArgs {
            preset,
            worktree: worktree && pre_selected_worktree.is_none(),
            checkout,
            checkout_branch: if checkout { branch.clone() } else { None },
            checkout_worktree,
            checkout_worktree_branch: resolved_cw_branch,
            attach: false,
            worktree_path: pre_selected_worktree,
            no_attach: false,
        },
        settings,
    )
}
