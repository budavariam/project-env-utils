//! Interactive wrapper around `dev-backend` that asks whether to open VS Code
//! and which UI worktree to use before starting the tmux session.
use anyhow::Result;

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
        return super::dev_backend::run(
            &super::dev_backend::DevBackendArgs {
                preset,
                worktree,
                checkout,
                checkout_branch: branch.clone(),
                checkout_worktree,
                checkout_worktree_branch: if checkout_worktree { branch } else { None },
                attach: true,
                worktree_branch: None,
                no_attach: false,
            },
            settings,
        );
    }

    let parent = repo_parent();
    let cfg = &settings.project.dev_backend;

    // --worktree: pick the branch now so it is known before VS Code prompts fire.
    let pre_selected_branch: Option<String> = if worktree {
        let all_repo_dirs: Vec<_> = cfg
            .window_backend
            .iter()
            .map(|p| parent.join(&p.repo))
            .collect();
        let primary_repo = &all_repo_dirs[0];
        let b = match crate::cmd::worktree::pick_existing_worktree_fzf(primary_repo) {
            Ok(b) => b,
            Err(_) => {
                // Primary repo has no worktrees — collect from all backend repos.
                let branches = crate::cmd::worktree::worktree_branches_any(&all_repo_dirs);
                if branches.is_empty() {
                    anyhow::bail!(
                        "No worktrees found in any backend repo. Use --checkout-worktree to create one."
                    );
                }
                let list = branches.join("\n");
                let mut fzf = std::process::Command::new("fzf")
                    .arg("--prompt=Worktree branch: ")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|e| anyhow::anyhow!("failed to spawn fzf: {}", e))?;
                if let Some(mut stdin) = fzf.stdin.take() {
                    use std::io::Write;
                    stdin.write_all(list.as_bytes()).ok();
                }
                let out = fzf
                    .wait_with_output()
                    .map_err(|e| anyhow::anyhow!("fzf failed: {}", e))?;
                let selected = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if selected.is_empty() {
                    anyhow::bail!("No branch selected.");
                }
                selected
            }
        };
        eprintln!("  → {}", b);
        eprintln!(
            "Session: {}_{}",
            cfg.session_name,
            crate::cmd::worktree::branch_to_safe(&b)
        );
        Some(b)
    } else {
        None
    };

    // --checkout-worktree: resolve branch before VS Code prompts fire.
    let resolved_cw_branch: Option<String> = if checkout_worktree {
        let primary_repo = parent.join(&cfg.window_backend[0].repo);
        let b = match branch.clone() {
            Some(b) => b,
            None => crate::cmd::worktree::pick_branch_fzf(&primary_repo)?,
        };
        eprintln!("  → branch: {}", b);
        Some(b)
    } else {
        None
    };

    let backend_branch_for_vscode: Option<String> = pre_selected_branch
        .as_deref()
        .map(crate::cmd::worktree::branch_to_safe)
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
        let ui_repo = &settings.project.dev_ui.repo;
        let ui_branch: Option<String> = if !ui_repo.is_empty() {
            if let Some(ref bb) = backend_branch_for_vscode {
                eprintln!("\nWhich UI? (backend is on '{}')", bb);
            } else {
                eprintln!("\nWhich UI?");
            }
            let sel =
                workspace::pick_group_worktree(&parent, std::slice::from_ref(ui_repo), "UI> ")?;
            eprintln!("  → {}", sel.as_deref().unwrap_or("(root checkout)"));
            sel
        } else {
            None
        };

        let mut group_args: Vec<(String, String)> = Vec::new();
        if let Some(bb) = backend_branch_for_vscode.filter(|s| !s.is_empty()) {
            group_args.push(("backend".to_string(), bb));
        }
        if let Some(ub) = ui_branch.filter(|s| !s.is_empty()) {
            group_args.push(("ui".to_string(), ub));
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

    super::dev_backend::run(
        &super::dev_backend::DevBackendArgs {
            preset,
            worktree,
            checkout,
            checkout_branch: branch.clone(),
            checkout_worktree,
            checkout_worktree_branch: resolved_cw_branch,
            attach: false,
            worktree_branch: pre_selected_branch,
            no_attach: false,
        },
        settings,
    )
}
