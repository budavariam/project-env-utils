//! Shared worktree helpers used by dev_ui and dev_backend.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Replace '/' with '_' in a branch name so it is safe in paths and session names.
pub fn branch_to_safe(branch: &str) -> String {
    branch.replace('/', "_")
}

/// Return the currently checked-out branch name for `repo_dir`.
pub fn current_branch(repo_dir: &Path) -> Result<String> {
    let out = Command::new("git")
        .args([
            "-C",
            &repo_dir.to_string_lossy(),
            "symbolic-ref",
            "--short",
            "HEAD",
        ])
        .output()
        .context("git symbolic-ref failed")?;
    if !out.status.success() {
        bail!(
            "Could not determine current branch in {}",
            repo_dir.display()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Stash uncommitted changes (if any) and checkout `branch` in `repo_dir`.
/// Returns `true` if a stash was created.
pub fn stash_and_checkout(repo_dir: &Path, branch: &str) -> Result<bool> {
    let repo_s = repo_dir.to_string_lossy();

    // Check for uncommitted changes.
    let status = Command::new("git")
        .args(["-C", &repo_s, "status", "--porcelain"])
        .output()
        .context("git status failed")?;
    let has_changes = !status.stdout.is_empty();

    if has_changes {
        eprintln!("  {} — stashing uncommitted changes...", repo_dir.display());
        let stash = Command::new("git")
            .args([
                "-C",
                &repo_s,
                "stash",
                "push",
                "--include-untracked",
                "-m",
                "penv auto-stash before checkout",
            ])
            .output()
            .context("git stash failed")?;
        if !stash.status.success() {
            bail!(
                "git stash failed in {}: {}",
                repo_dir.display(),
                String::from_utf8_lossy(&stash.stderr).trim()
            );
        }
    }

    let checkout = Command::new("git")
        .args(["-C", &repo_s, "checkout", branch])
        .output()
        .context("git checkout failed")?;
    if !checkout.status.success() {
        bail!(
            "git checkout '{}' failed in {}: {}",
            branch,
            repo_dir.display(),
            String::from_utf8_lossy(&checkout.stderr).trim()
        );
    }

    Ok(has_changes)
}

/// Standard worktree path for a branch inside a repo.
/// Convention: `{repo_dir}/.claude/worktrees/{branch_safe}`
pub fn worktree_path(repo_dir: &Path, branch: &str) -> PathBuf {
    repo_dir
        .join(".claude")
        .join("worktrees")
        .join(branch_to_safe(branch))
}

/// Pick an existing worktree interactively using the `git wtf` alias.
/// Returns `(worktree_path, branch_name)`.
pub fn pick_worktree_wtf(repo_dir: &Path) -> Result<(PathBuf, String)> {
    let repo_s = repo_dir.to_string_lossy();
    let check = Command::new("git")
        .args(["-C", &repo_s, "wtf", "--help"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !check.map(|s| s.success()).unwrap_or(false) {
        bail!("'git wtf' alias not configured. Add it to ~/.gitconfig:\n  [alias]\n    wtf = !...");
    }

    let out = Command::new("git")
        .args(["-C", &repo_s, "wtf"])
        .output()
        .context("git wtf failed")?;
    let dir_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if dir_str.is_empty() {
        bail!("No worktree selected.");
    }
    let dir = PathBuf::from(&dir_str);

    let branch = Command::new("git")
        .args(["-C", &dir_str, "branch", "--show-current"])
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        })
        .unwrap_or_else(|| {
            dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        });

    eprintln!("Workspace: {} (branch: {})", dir_str, branch);
    Ok((dir, branch))
}

/// Pick one item from `choices` using fzf. Used when multiple tmux sessions match.
pub fn pick_session_fzf(choices: &[String]) -> Result<String> {
    use std::io::Write;
    let mut fzf = std::process::Command::new("fzf")
        .args(["--prompt=Session: "])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn fzf — is it installed?")?;

    if let Some(mut stdin) = fzf.stdin.take() {
        stdin.write_all(choices.join("\n").as_bytes()).ok();
    }

    let out = fzf.wait_with_output().context("fzf failed")?;
    let selected = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if selected.is_empty() {
        bail!("No session selected.");
    }
    Ok(selected)
}

/// Pick a branch interactively using fzf.
pub fn pick_branch_fzf(repo_dir: &Path) -> Result<String> {
    let repo_s = repo_dir.to_string_lossy();
    let git_out = Command::new("git")
        .args(["-C", &repo_s, "branch", "--format=%(refname:short)"])
        .output()
        .context("git branch failed")?;
    let branch_list = String::from_utf8_lossy(&git_out.stdout).into_owned();

    let mut fzf = Command::new("fzf")
        .args(["--prompt=Branch: "])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn fzf — is it installed?")?;

    if let Some(mut stdin) = fzf.stdin.take() {
        stdin.write_all(branch_list.as_bytes()).ok();
    }

    let fzf_out = fzf.wait_with_output().context("fzf failed")?;
    let selected = String::from_utf8_lossy(&fzf_out.stdout).trim().to_string();
    if selected.is_empty() {
        bail!("No branch selected.");
    }
    Ok(selected)
}

/// Pick an existing worktree from `{repo}/.claude/worktrees/` using fzf.
/// Returns the branch name (read from git inside the worktree).
pub fn pick_existing_worktree_fzf(repo_dir: &Path) -> Result<String> {
    let wt_base = repo_dir.join(".claude").join("worktrees");
    if !wt_base.exists() {
        bail!(
            "No worktrees found in {}. Use --checkout-worktree to create one.",
            wt_base.display()
        );
    }

    let mut entries: Vec<String> = std::fs::read_dir(&wt_base)
        .context("failed to read worktrees directory")?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    entries.sort();

    if entries.is_empty() {
        bail!(
            "No worktrees found in {}. Use --checkout-worktree to create one.",
            wt_base.display()
        );
    }

    let list = entries.join("\n");
    let mut fzf = Command::new("fzf")
        .args(["--prompt=Worktree: "])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn fzf — is it installed?")?;

    if let Some(mut stdin) = fzf.stdin.take() {
        stdin.write_all(list.as_bytes()).ok();
    }

    let out = fzf.wait_with_output().context("fzf failed")?;
    let selected = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if selected.is_empty() {
        bail!("No worktree selected.");
    }

    let wt_path = wt_base.join(&selected);
    let branch = Command::new("git")
        .args(["-C", &wt_path.to_string_lossy(), "branch", "--show-current"])
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        })
        .unwrap_or(selected);

    Ok(branch)
}

/// Ensure a worktree exists at the standard path for the given repo and branch.
/// Creates it if missing.  Returns the worktree path.
/// If the branch is already checked out in another worktree (including the main repo),
/// that existing path is returned instead of failing.
pub fn ensure_worktree(repo_dir: &Path, branch: &str) -> Result<PathBuf> {
    let wt = worktree_path(repo_dir, branch);
    if wt.exists() {
        eprintln!("Resuming existing worktree: {}", wt.display());
        return Ok(wt);
    }

    let repo_s = repo_dir.to_string_lossy();
    let wt_s = wt.to_string_lossy();
    let output = Command::new("git")
        .args(["-C", &repo_s, "worktree", "add", &wt_s, branch])
        .output()
        .context("git worktree add failed")?;

    if output.status.success() {
        return Ok(wt);
    }

    // Branch already checked out somewhere — find that path via worktree list.
    if let Some(existing) = find_worktree_for_branch(repo_dir, branch)? {
        // If the existing checkout is the main repo itself, refuse to use it —
        // the whole point of worktrees is to leave the main repo free for manual branch switching.
        if existing == repo_dir {
            bail!(
                "Branch '{}' is checked out in the main repo ({}). \
                 Checkout a different branch there first, then retry.",
                branch,
                repo_dir.display()
            );
        }
        eprintln!(
            "Branch '{}' already checked out at: {}",
            branch,
            existing.display()
        );
        return Ok(existing);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "git worktree add failed for '{}' in '{}': {}",
        branch,
        repo_dir.display(),
        stderr.trim()
    )
}

/// Parse `git worktree list --porcelain` and return the path where `branch` is checked out.
fn find_worktree_for_branch(repo_dir: &Path, branch: &str) -> Result<Option<PathBuf>> {
    let repo_s = repo_dir.to_string_lossy();
    let out = Command::new("git")
        .args(["-C", &repo_s, "worktree", "list", "--porcelain"])
        .output()
        .context("git worktree list failed")?;

    let text = String::from_utf8_lossy(&out.stdout);
    let full_ref = format!("refs/heads/{}", branch);

    let mut current_path: Option<PathBuf> = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path));
        } else if line == format!("branch {}", full_ref) {
            if let Some(p) = current_path {
                return Ok(Some(p));
            }
        } else if line.is_empty() {
            current_path = None;
        }
    }
    Ok(None)
}

/// Write a close_session-only helper script.
pub fn write_close_session_script(
    path: &str,
    session: &str,
    penv: &str,
    preset: &str,
    services: &[&str],
    show_info_cmd: &str,
) -> Result<()> {
    let reload_cmd = services
        .iter()
        .map(|s| format!("'{}' load-env '{}' '{}'", penv, preset, s))
        .collect::<Vec<_>>()
        .join(" && ");
    let change_preset_cmd = services
        .iter()
        .map(|s| format!("'{}'", s))
        .collect::<Vec<_>>()
        .join(" ");
    let content = format!(
        "reload_env() {{ {}; }}\nchange_preset() {{ '{}' change-preset {}; }}\nclose_session() {{ tmux kill-session -t \"={}\"; }}\nshow_info() {{ {}; }}\ninspect_env() {{ '{}' env-age --preset '{}' \"$@\"; }}\necho \"Run reload_env to reload .env, change_preset to switch preset, or close_session to close the session.\"\n",
        reload_cmd, penv, change_preset_cmd, session, show_info_cmd, penv, preset
    );
    std::fs::write(path, content)
        .with_context(|| format!("failed to write helper script to {}", path))?;
    Ok(())
}

/// Write a teardown (remove worktrees) + close_session helper script.
/// `worktrees`: list of (worktree_path, parent_repo_dir) to remove.
pub fn write_teardown_script(
    path: &str,
    session: &str,
    worktrees: &[(&Path, &Path)],
    penv: &str,
    preset: &str,
    services: &[&str],
    show_info_cmd: &str,
) -> Result<()> {
    let pane_id_fmt = "#{pane_id}";
    let mut lines: Vec<String> = vec![
        "teardown() {\n".to_string(),
        "  echo \"Stopping processes in worktrees...\"\n".to_string(),
        format!(
            "  tmux list-panes -t \"={}\" -F \"{}\" 2>/dev/null | while read -r pane; do\n",
            session, pane_id_fmt
        ),
        format!(
            "    [[ \"$pane\" != \"$(tmux display-message -p '{}')\" ]] && \\\n",
            pane_id_fmt
        ),
        "      tmux send-keys -t \"$pane\" C-c \"\" 2>/dev/null || true\n".to_string(),
        "  done\n".to_string(),
        "  sleep 2\n".to_string(),
    ];

    for (wt, _) in worktrees {
        lines.push(format!(
            "  pkill -f \"{}\" 2>/dev/null || true\n",
            wt.to_string_lossy()
        ));
    }

    lines.push("  echo \"Removing worktrees...\"\n".to_string());
    for (wt, repo) in worktrees {
        lines.push(format!(
            "  git -C \"{}\" worktree remove \"{}\" --force\n",
            repo.to_string_lossy(),
            wt.to_string_lossy()
        ));
    }
    lines.push(format!("  tmux kill-session -t \"={}\"\n", session));
    lines.push("}\n".to_string());
    lines.push(format!(
        "close_session() {{ tmux kill-session -t \"={}\"; }}\n",
        session
    ));
    let reload_cmd = services
        .iter()
        .map(|s| format!("'{}' load-env '{}' '{}'", penv, preset, s))
        .collect::<Vec<_>>()
        .join(" && ");
    let change_preset_services = services
        .iter()
        .map(|s| format!("'{}'", s))
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(format!("reload_env() {{ {}; }}\n", reload_cmd));
    lines.push(format!(
        "change_preset() {{ '{}' change-preset {}; }}\n",
        penv, change_preset_services
    ));
    lines.push(format!("show_info() {{ {}; }}\n", show_info_cmd));
    lines.push(format!(
        "inspect_env() {{ '{}' env-age --preset '{}' \"$@\"; }}\n",
        penv, preset
    ));
    lines.push(
        format!(
            "echo \"Run teardown to remove worktrees & close session, reload_env to reload .env from preset '{}', change_preset to switch preset, or close_session to just close it.\"\n",
            preset
        )
    );

    let content = lines.concat();
    std::fs::write(path, content)
        .with_context(|| format!("failed to write teardown script to {}", path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_to_safe_replaces_slashes() {
        assert_eq!(branch_to_safe("feat/my-feature"), "feat_my-feature");
        assert_eq!(branch_to_safe("main"), "main");
        assert_eq!(branch_to_safe("fix/issue/123"), "fix_issue_123");
    }

    #[test]
    fn worktree_path_uses_claude_worktrees() {
        let repo = PathBuf::from("/repos/my-service");
        let path = worktree_path(&repo, "feat/foo");
        assert_eq!(
            path,
            PathBuf::from("/repos/my-service/.claude/worktrees/feat_foo")
        );
    }

    #[test]
    fn close_session_script_has_session_name() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("h.sh").to_string_lossy().into_owned();
        write_close_session_script(
            &p,
            "my-session",
            "/usr/local/bin/penv",
            "test",
            &["my-api"],
            "penv show-info",
        )
        .unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.contains("close_session"));
        assert!(s.contains("my-session"));
        assert!(s.contains("reload_env"));
        assert!(s.contains("show_info"));
        assert!(!s.contains("teardown"));
    }

    #[test]
    fn teardown_script_contains_all_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("h.sh").to_string_lossy().into_owned();
        let wt1 = PathBuf::from("/repos/svc-a/.claude/worktrees/feat_x");
        let repo1 = PathBuf::from("/repos/svc-a");
        let wt2 = PathBuf::from("/repos/svc-b/.claude/worktrees/feat_x");
        let repo2 = PathBuf::from("/repos/svc-b");
        write_teardown_script(
            &p,
            "my-session",
            &[(&wt1, &repo1), (&wt2, &repo2)],
            "/usr/local/bin/penv",
            "test",
            &["svc-a", "svc-b"],
            "penv show-info",
        )
        .unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.contains("teardown"));
        assert!(s.contains("close_session"));
        assert!(s.contains("reload_env"));
        assert!(s.contains("show_info"));
        assert!(s.contains("svc-a"));
        assert!(s.contains("svc-b"));
    }
}
