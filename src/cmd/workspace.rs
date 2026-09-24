use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use crate::config::Settings;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ── Config structs ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default, Clone)]
pub struct VscodeConfig {
    pub output_file: Option<String>,
    pub peacock_color: Option<String>,
    pub extra_settings: Option<Value>,
    #[serde(default)]
    pub extra_folders: Vec<WorkspaceFolder>,
    #[serde(default)]
    pub presets: Vec<VscodePreset>,
    /// Named repo groups used with --group <name>=<branch>.
    /// When empty, "ui" and "backend" are auto-derived from dev_ui / dev_backend.
    #[serde(default)]
    pub groups: Vec<VscodeGroup>,
}

/// A named group of repos for use with `--group <name>=<branch>`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct VscodeGroup {
    pub name: String,
    pub repos: Vec<String>,
}

/// A named subset of repos to open together.
/// Each entry is either a bare string (service name or extra_folder path/name)
/// or an object with a per-folder worktree branch override.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct VscodePreset {
    pub name: String,
    pub folders: Vec<FolderKey>,
    /// Per-preset Peacock color. Overrides vscode.peacock_color when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peacock_color: Option<String>,
}

/// A folder reference inside a preset.
///
/// Simple:     `"demo-ui"`                               — use global --branch (or root)
/// WithBranch: `{"repo": "demo-ui", "branch": "feat_x"}` — fixed branch, ignores --branch
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum FolderKey {
    Simple(String),
    WithBranch { repo: String, branch: String },
}

impl FolderKey {
    pub fn key(&self) -> &str {
        match self {
            FolderKey::Simple(s) => s,
            FolderKey::WithBranch { repo, .. } => repo,
        }
    }

    pub fn explicit_branch(&self) -> Option<&str> {
        match self {
            FolderKey::Simple(_) => None,
            FolderKey::WithBranch { branch, .. } => Some(branch),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct WorkspaceFolder {
    pub name: Option<String>,
    pub path: String,
}

// ── Config I/O ──────────────────────────────────────────────────────────────

pub fn load_vscode_config() -> Result<VscodeConfig> {
    let path = crate::config::project_config_file();
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let val: Value = serde_json::from_str(&raw).context("failed to parse settings.json")?;

    match val.get("vscode") {
        Some(v) => serde_json::from_value::<VscodeConfig>(v.clone())
            .context("invalid 'vscode' block in settings.json"),
        None => Ok(VscodeConfig::default()),
    }
}

/// Append a preset to vscode.presets in settings.json (in-place JSON edit).
fn save_preset_to_settings(preset: &VscodePreset) -> Result<()> {
    let path = crate::config::project_config_file();
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let mut val: Value = serde_json::from_str(&raw).context("failed to parse settings.json")?;

    let vscode = val
        .as_object_mut()
        .context("settings.json root is not an object")?
        .entry("vscode")
        .or_insert_with(|| json!({}));

    let presets = vscode
        .as_object_mut()
        .context("vscode block is not an object")?
        .entry("presets")
        .or_insert_with(|| json!([]));

    presets
        .as_array_mut()
        .context("vscode.presets is not an array")?
        .push(serde_json::to_value(preset)?);

    let json = serde_json::to_string_pretty(&val)?;
    std::fs::write(&path, format!("{}\n", json))
        .with_context(|| format!("failed to write {}", path.display()))
}

// ── Open ────────────────────────────────────────────────────────────────────

/// Return effective groups: explicit config if set, otherwise auto-derive from dev_ui/dev_backend.
fn resolve_groups(cfg: &VscodeConfig, settings: &Settings) -> Vec<VscodeGroup> {
    if !cfg.groups.is_empty() {
        return cfg.groups.clone();
    }
    let mut groups = Vec::new();
    let ui = &settings.project.dev_ui.repo;
    if !ui.is_empty() {
        groups.push(VscodeGroup {
            name: "ui".to_string(),
            repos: vec![ui.clone()],
        });
    }
    let backend: Vec<String> = settings
        .project
        .dev_backend
        .window_backend
        .iter()
        .map(|p| p.repo.clone())
        .collect();
    if !backend.is_empty() {
        groups.push(VscodeGroup {
            name: "backend".to_string(),
            repos: backend,
        });
    }
    groups
}

/// Parse `--group` values of the form `name=branch` into `(name, branch)` pairs.
pub fn parse_group_args(raw: &[String]) -> anyhow::Result<Vec<(String, String)>> {
    raw.iter()
        .map(|s| {
            let (name, branch) = s.split_once('=').ok_or_else(|| {
                anyhow::anyhow!(
                    "--group value '{}' must be in the form <name>=<branch>  e.g. ui=feat_my-feature",
                    s
                )
            })?;
            if name.is_empty() || branch.is_empty() {
                anyhow::bail!("--group '{}': name and branch must both be non-empty", s);
            }
            Ok((name.to_string(), branch.to_string()))
        })
        .collect()
}

/// Generate and open a VS Code workspace.
///
/// Folder list resolution order:
///   1. Named preset (--preset) if given, else all services + all extra_folders
///   2. Per-folder explicit branch (FolderKey::WithBranch in preset) — highest priority
///   3. --group <name>=<branch> — applies to repos in the named group
///   4. --branch <B> / --worktree — fallback for remaining service repos
///   3. extra_folders entries keep their configured name/path
#[allow(clippy::too_many_arguments)]
pub fn run_open(
    settings: &Settings,
    preset_name: Option<&str>,
    worktree: bool,
    branch: Option<&str>,
    group_args: &[(String, String)],
    peacock_color_override: Option<&str>,
    no_open: bool,
    ff: bool,
) -> Result<()> {
    let cfg = load_vscode_config()?;
    let parent = crate::config::repo_parent();

    let service_names: Vec<String> = settings
        .project
        .services
        .iter()
        .map(|s| s.name.clone())
        .collect();

    // Resolve groups (explicit config or auto-derived from dev_ui / dev_backend)
    let groups = resolve_groups(&cfg, settings);

    // Validate group names from CLI args
    for (name, _) in group_args {
        if !groups.iter().any(|g| &g.name == name) {
            let known: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
            anyhow::bail!(
                "Unknown group '{}'. Known groups: {}",
                name,
                known.join(", ")
            );
        }
    }

    // Build a repo → branch lookup from --group args
    let repo_branch: std::collections::HashMap<String, String> = groups
        .iter()
        .filter_map(|g| {
            group_args
                .iter()
                .find(|(name, _)| name == &g.name)
                .map(|(_, branch)| (g, branch))
        })
        .flat_map(|(g, b)| g.repos.iter().map(move |r| (r.clone(), b.clone())))
        .collect();

    // Determine which folder keys to include
    let folder_keys: Vec<FolderKey> = if let Some(pname) = preset_name {
        let p = cfg
            .presets
            .iter()
            .find(|p| p.name == pname)
            .ok_or_else(|| {
                let avail = cfg
                    .presets
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::anyhow!(
                    "Workspace preset '{}' not found.{}",
                    pname,
                    if avail.is_empty() {
                        " No presets saved yet — use 'workspace wizard --save <name>'.".to_string()
                    } else {
                        format!(" Available: {}", avail)
                    }
                )
            })?;
        p.folders.clone()
    } else {
        // All services + all extra_folders (by path, falling back to name)
        let mut keys: Vec<FolderKey> = service_names
            .iter()
            .map(|s| FolderKey::Simple(s.clone()))
            .collect();
        for f in &cfg.extra_folders {
            keys.push(FolderKey::Simple(
                f.name.clone().unwrap_or_else(|| f.path.clone()),
            ));
        }
        keys
    };

    // Resolve the global default branch (used for repos not covered by --group)
    let default_wt_branch: Option<String> = if worktree || branch.is_some() {
        let b = match branch {
            Some(b) => b.to_string(),
            None => pick_worktree_branch(&parent, &service_names, ff)?,
        };
        Some(b)
    } else {
        None
    };

    // Track branches used (for the output filename)
    let mut any_wt_safe: Option<String> = None;
    let mut all_wt_safes: std::collections::BTreeSet<String> = Default::default();

    // Resolve each key to a WorkspaceFolder
    let folders: Vec<WorkspaceFolder> = folder_keys
        .iter()
        .map(|fk| {
            let key = fk.key();

            // Is it a service repo?
            if service_names.contains(&key.to_string()) {
                // Priority: per-folder explicit branch > --group branch > global default
                let effective_branch = fk
                    .explicit_branch()
                    .map(str::to_string)
                    .filter(|s| !s.is_empty())
                    .or_else(|| repo_branch.get(key).cloned().filter(|s| !s.is_empty()))
                    .or_else(|| default_wt_branch.clone().filter(|s| !s.is_empty()));

                if let Some(ref raw_branch) = effective_branch {
                    let safe = crate::cmd::worktree::branch_to_safe(raw_branch);
                    let wt_path = format!("{}/.claude/worktrees/{}", key, safe);
                    if !parent.join(&wt_path).exists() {
                        eprintln!(
                            "Warning: worktree not found: {}",
                            parent.join(&wt_path).display()
                        );
                    }
                    if any_wt_safe.is_none() {
                        any_wt_safe = Some(safe.clone());
                    }
                    all_wt_safes.insert(safe.clone());
                    return WorkspaceFolder {
                        name: Some(format!("{} [{}]", key, safe)),
                        path: wt_path,
                    };
                }

                return WorkspaceFolder {
                    name: Some(key.to_string()),
                    path: key.to_string(),
                };
            }

            // Is it an extra_folder (match by name or by path)?
            if let Some(ef) = cfg
                .extra_folders
                .iter()
                .find(|f| f.name.as_deref() == Some(key) || f.path == key)
            {
                return ef.clone();
            }

            // Fallback: treat the key as a bare path
            WorkspaceFolder {
                name: None,
                path: key.to_string(),
            }
        })
        .collect();

    // Color priority: explicit override > preset color > global vscode.peacock_color
    let preset_color = preset_name
        .and_then(|pname| cfg.presets.iter().find(|p| p.name == pname))
        .and_then(|p| p.peacock_color.as_deref());
    let effective_color = peacock_color_override
        .or(preset_color)
        .or(cfg.peacock_color.as_deref());

    let ws_json = build_workspace_json(&folders, effective_color, cfg.extra_settings.as_ref());

    // When using worktrees, embed the branch(es) in the filename so each
    // combination gets its own persistent entry in VS Code's recent workspaces list.
    let out_path = if !all_wt_safes.is_empty() {
        let base = cfg
            .output_file
            .as_deref()
            .unwrap_or("workspace.code-workspace");
        let stem = base.trim_end_matches(".code-workspace");
        let branch_suffix = if all_wt_safes.len() == 1 {
            all_wt_safes.iter().next().unwrap().clone()
        } else {
            all_wt_safes.iter().cloned().collect::<Vec<_>>().join("+")
        };
        parent.join(format!("{}-{}.code-workspace", stem, branch_suffix))
    } else {
        parent.join(
            cfg.output_file
                .as_deref()
                .unwrap_or("workspace.code-workspace"),
        )
    };

    let ws_json = merge_existing_settings(&out_path, ws_json);
    write_workspace(&out_path, &ws_json)?;
    eprintln!("Wrote: {}", out_path.display());

    if !no_open {
        open_workspace(&out_path)?;
        eprintln!("Opened: {}", out_path.display());
    }

    Ok(())
}

// ── Wizard ──────────────────────────────────────────────────────────────────

/// Interactive wizard: fzf multiselect from all known repos, then write workspace.
/// With --save, also persists the selection as a named preset in settings.json.
pub fn run_wizard(
    settings: &Settings,
    output: Option<&str>,
    open: bool,
    save_as: Option<&str>,
) -> Result<()> {
    let cfg = load_vscode_config()?;

    // Validate preset name uniqueness before we do anything interactive
    if save_as.is_some_and(|name| cfg.presets.iter().any(|p| p.name == name)) {
        bail!(
            "A preset named '{}' already exists in settings.json vscode.presets.\n\
             Choose a different name or remove the existing preset first.",
            save_as.unwrap()
        );
    }

    let parent = crate::config::repo_parent();
    let service_names: Vec<String> = settings
        .project
        .services
        .iter()
        .map(|s| s.name.clone())
        .collect();

    // Build the full candidate list: services first, then extras, then everything else in parent
    let mut all_keys: Vec<String> = service_names.clone();
    for f in &cfg.extra_folders {
        let key = f.name.clone().unwrap_or_else(|| f.path.clone());
        if !all_keys.contains(&key) {
            all_keys.push(key);
        }
    }
    // Scan parent dir for any remaining directories not already listed
    if let Ok(entries) = std::fs::read_dir(&parent) {
        let mut extras: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.starts_with('.') && !all_keys.contains(n))
            .collect();
        extras.sort();
        all_keys.extend(extras);
    }

    eprintln!("Select repos — TAB to multi-select, Enter to confirm:");
    let selected = fzf_multiselect(&all_keys, "repos> ")?;
    if selected.is_empty() {
        bail!("No repos selected.");
    }

    // Resolve selected keys to WorkspaceFolders
    let folders: Vec<WorkspaceFolder> = selected
        .iter()
        .map(|key| {
            if let Some(ef) = cfg
                .extra_folders
                .iter()
                .find(|f| f.name.as_deref() == Some(key) || f.path == *key)
            {
                return ef.clone();
            }
            WorkspaceFolder {
                name: if service_names.contains(key) {
                    Some(key.clone())
                } else {
                    None
                },
                path: key.clone(),
            }
        })
        .collect();

    let suggested_color = random_peacock_color();
    let color_input = prompt(&format!(
        "Peacock color [Enter for random {}, or type #rrggbb to override]: ",
        suggested_color
    ))?;
    let chosen_color = if color_input.is_empty() {
        suggested_color
    } else {
        color_input
    };

    let ws_json = build_workspace_json(&folders, Some(&chosen_color), cfg.extra_settings.as_ref());

    let out_path = if let Some(o) = output {
        PathBuf::from(o)
    } else {
        let filename = save_as
            .map(|n| format!("{}.code-workspace", n.to_lowercase().replace(' ', "-")))
            .or_else(|| cfg.output_file.clone())
            .unwrap_or_else(|| "workspace.code-workspace".to_string());
        parent.join(filename)
    };

    let ws_json = merge_existing_settings(&out_path, ws_json);
    write_workspace(&out_path, &ws_json)?;
    eprintln!("Wrote: {}", out_path.display());

    // Persist preset if --save was given
    if let Some(name) = save_as {
        let preset = VscodePreset {
            name: name.to_string(),
            folders: selected.into_iter().map(FolderKey::Simple).collect(),
            peacock_color: Some(chosen_color),
        };
        save_preset_to_settings(&preset)?;
        eprintln!("Saved preset '{}' to settings.json", name);
    }

    let should_open = if open {
        true
    } else {
        let ans = prompt("Open workspace now? [y/N]: ")?;
        ans.eq_ignore_ascii_case("y")
    };

    if should_open {
        open_workspace(&out_path)?;
    }

    Ok(())
}

// ── Guide ────────────────────────────────────────────────────────────────────

/// Interactive guide that walks through workspace options and opens the result.
/// Prints the equivalent `workspace open` command so the user can bookmark it.
pub fn run_guide(settings: &Settings) -> Result<()> {
    let cfg = load_vscode_config()?;
    let parent = crate::config::repo_parent();
    let groups = resolve_groups(&cfg, settings);

    // ── Step 1: preset ────────────────────────────────────────────────────
    eprintln!("Step 1/3 — Which repos do you want to open?");
    let mut preset_choices = vec!["All repos (services + extras)".to_string()];
    for p in &cfg.presets {
        preset_choices.push(format!("Preset: {} ({} repos)", p.name, p.folders.len()));
    }
    let preset_sel = fzf_select(&preset_choices, "Repos> ")?;
    eprintln!("  → {}", preset_sel);
    let preset_name: Option<String> = if preset_sel.starts_with("Preset: ") {
        Some(
            preset_sel
                .trim_start_matches("Preset: ")
                .split(" (")
                .next()
                .unwrap_or("")
                .to_string(),
        )
    } else {
        None
    };

    // ── Step 2: worktree mode ─────────────────────────────────────────────
    eprintln!("\nStep 2/3 — Worktree mode?");
    let mode_choices = vec![
        "Root — all repos at their current checkout".to_string(),
        "Same worktree — all service repos on the same branch".to_string(),
        "Per group — different branches per group (ui / backend / ...)".to_string(),
    ];
    let mode_sel = fzf_select(&mode_choices, "Mode> ")?;
    eprintln!("  → {}", mode_sel);

    // ── Step 3: branch selection ──────────────────────────────────────────
    let mut branch: Option<String> = None;
    let mut group_args: Vec<(String, String)> = Vec::new();
    let mut group_root: Vec<String> = Vec::new();

    if mode_sel.starts_with("Same worktree") {
        eprintln!("\nStep 3/3 — Pick the worktree branch:");
        let service_names: Vec<String> = settings
            .project
            .services
            .iter()
            .map(|s| s.name.clone())
            .collect();
        let b = pick_worktree_branch(&parent, &service_names, false)?;
        eprintln!("  → {}", b);
        branch = Some(b);
    } else if mode_sel.starts_with("Per group") {
        eprintln!(
            "\nStep 3/3 — Pick a branch for each group (or '(root checkout)' to keep at current):"
        );
        for g in &groups {
            let repo = match g.repos.first() {
                Some(r) => r,
                None => continue,
            };
            let wt_base = parent.join(repo).join(".claude").join("worktrees");
            let mut choices = vec!["\t(root checkout)".to_string()];
            if wt_base.exists() {
                let mut entries: Vec<String> = std::fs::read_dir(&wt_base)
                    .unwrap_or_else(|_| std::fs::read_dir("/dev/null").unwrap())
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect();
                entries.sort();
                for name in entries {
                    let wt_path = wt_base.join(&name);
                    let date = Command::new("git")
                        .args([
                            "-C",
                            &wt_path.to_string_lossy(),
                            "log",
                            "-1",
                            "--format=%cr",
                            "HEAD",
                        ])
                        .output()
                        .ok()
                        .and_then(|o| {
                            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                            if s.is_empty() { None } else { Some(s) }
                        })
                        .unwrap_or_else(|| "unknown".to_string());
                    choices.push(format!("{}\t{}", date, name));
                }
            }
            let raw = fzf_select_tab(&choices, &format!("  {} branch> ", g.name))?;
            let sel = raw.split('\t').nth(1).unwrap_or("").trim().to_string();
            eprintln!("  → {}: {}", g.name, sel);
            if sel == "(root checkout)" || sel.is_empty() {
                group_root.push(g.name.clone());
            } else {
                group_args.push((g.name.clone(), sel));
            }
        }
    } else {
        eprintln!("\nStep 3/3 — (skipped — all repos at root)");
    }

    // ── Step 4: peacock color ─────────────────────────────────────────────
    eprintln!("\nStep 4/4 — Peacock color for this workspace?");
    let current_color = preset_name
        .as_deref()
        .and_then(|pname| cfg.presets.iter().find(|p| p.name == pname))
        .and_then(|p| p.peacock_color.as_deref())
        .or(cfg.peacock_color.as_deref());

    let peacock_color_override = pick_peacock_color(current_color)?;

    // ── Summary ───────────────────────────────────────────────────────────
    eprintln!("\nSummary:");
    eprintln!("  Repos:  {}", preset_name.as_deref().unwrap_or("all"));
    if branch.is_some() || !group_args.is_empty() || !group_root.is_empty() {
        for (name, b) in &group_args {
            eprintln!("  {:8} → worktree: {}", name, b);
        }
        for name in &group_root {
            eprintln!("  {:8} → root checkout", name);
        }
        if let Some(ref b) = branch {
            eprintln!("  all     → worktree: {}", b);
        }
    } else {
        eprintln!("  Mode:   root checkout (all repos)");
    }
    eprintln!(
        "  Color:  {}",
        peacock_color_override
            .as_deref()
            .unwrap_or(current_color.unwrap_or("(VS Code default)"))
    );

    // ── Print equivalent command ──────────────────────────────────────────
    let mut cmd_parts = vec!["penv workspace open".to_string()];
    if let Some(ref p) = preset_name {
        cmd_parts.push(format!("--preset {}", p));
    }
    if let Some(ref b) = branch {
        cmd_parts.push(format!("--branch {}", b));
    }
    for (name, b) in &group_args {
        cmd_parts.push(format!("--group {}={}", name, b));
    }
    if let Some(ref c) = peacock_color_override {
        cmd_parts.push(format!("--peacock-color {}", c));
    }
    eprintln!("\nEquivalent command:\n  {}\n", cmd_parts.join(" "));

    // ── Execute ───────────────────────────────────────────────────────────
    run_open(
        settings,
        preset_name.as_deref(),
        branch.is_some(),
        branch.as_deref(),
        &group_args,
        peacock_color_override.as_deref(),
        false,
        false,
    )
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Interactively pick a Peacock color. Returns `None` to keep `current`,
/// or `Some(hex)` for a new selection.
pub fn pick_peacock_color(current: Option<&str>) -> Result<Option<String>> {
    let random_hex = random_peacock_color();
    let mut color_choices = vec![
        match current {
            Some(c) => format!("{} Keep current  {}", color_swatch(c), c),
            None => "      Keep current  (none set)".to_string(),
        },
        format!(
            "{} {}  Random generated",
            color_swatch(&random_hex),
            random_hex
        ),
    ];
    for i in 0u32..10 {
        let hue = i as f32 * 36.0;
        let hex = hsl_to_hex(hue, 0.45, 0.48);
        color_choices.push(format!("{} {}  {}", color_swatch(&hex), hex, hue_name(hue)));
    }
    for (hex, label) in &[
        ("#000000", "black"),
        ("#404040", "dark gray"),
        ("#808080", "mid gray"),
        ("#c0c0c0", "light gray"),
        ("#ffffff", "white"),
    ] {
        color_choices.push(format!("{} {}  {}", color_swatch(hex), hex, label));
    }
    color_choices.push("      Custom hex  (type 6 hex chars in the next step, no #)".to_string());

    let color_sel = fzf_select_ansi(&color_choices, "Color> ")?;
    let color_sel_plain = strip_ansi(&color_sel);
    eprintln!("  → {}", color_sel_plain.trim());

    if color_sel_plain.contains("Keep current") {
        Ok(None)
    } else if color_sel_plain.contains("Random generated") {
        Ok(color_sel_plain
            .split_whitespace()
            .find(|w| w.starts_with('#') && w.len() == 7)
            .map(str::to_string))
    } else if color_sel_plain.contains("Custom hex") {
        let raw = prompt("  Hex color (6 chars, no #): ")?;
        let hex = format!("#{}", raw.trim().trim_start_matches('#'));
        if parse_hex_color(&hex).is_none() {
            eprintln!(
                "  Warning: '{}' is not a valid hex color — skipping color override",
                hex
            );
            Ok(None)
        } else {
            eprintln!("  Using: {}", hex);
            Ok(Some(hex))
        }
    } else {
        Ok(color_sel_plain
            .split_whitespace()
            .find(|w| w.starts_with('#') && w.len() == 7)
            .map(str::to_string))
    }
}

fn hue_name(hue: f32) -> &'static str {
    match ((hue / 36.0).round() as u32) % 10 {
        0 => "red",
        1 => "orange",
        2 => "yellow",
        3 => "chartreuse",
        4 => "green",
        5 => "teal",
        6 => "cyan",
        7 => "blue",
        8 => "purple",
        9 => "pink",
        _ => "color",
    }
}

/// Parse `#rrggbb` into (r, g, b). Returns None on bad input.
fn parse_hex_color(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Build a 5-char wide color swatch using 24-bit background ANSI codes.
fn color_swatch(hex: &str) -> String {
    match parse_hex_color(hex) {
        Some((r, g, b)) => format!("\x1b[48;2;{};{};{}m     \x1b[0m", r, g, b),
        None => "     ".to_string(),
    }
}

/// Strip ANSI escape sequences from a string.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for nc in chars.by_ref() {
                if nc.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// fzf single-select with `--ansi` so items can contain ANSI color codes.
fn fzf_select_ansi(items: &[String], prompt_str: &str) -> Result<String> {
    let mut fzf = Command::new("fzf")
        .args(["--ansi", "--prompt", prompt_str])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn fzf — is it installed?")?;

    if let Some(mut stdin) = fzf.stdin.take() {
        stdin.write_all(items.join("\n").as_bytes()).ok();
    }

    let out = fzf.wait_with_output().context("fzf wait failed")?;
    let selected = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if selected.is_empty() {
        bail!("Nothing selected.");
    }
    Ok(selected)
}

fn pick_worktree_branch(parent: &Path, service_repos: &[String], ff: bool) -> Result<String> {
    let repo = service_repos
        .first()
        .ok_or_else(|| anyhow::anyhow!("No services defined in settings.json"))?;

    let wt_base = parent.join(repo).join(".claude").join("worktrees");
    if !wt_base.exists() {
        bail!(
            "No worktrees found in {}.\n\
             Create one first with: penv dev-ui --checkout-worktree --branch <name>",
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
        bail!("No worktree entries found in {}", wt_base.display());
    }
    if ff && entries.len() == 1 {
        eprintln!("Fast-forwarding to only worktree: {}", entries[0]);
        return Ok(entries[0].clone());
    }

    // Build display list with last commit dates.
    let items: Vec<String> = entries
        .iter()
        .map(|name| {
            let wt_path = wt_base.join(name);
            let date = std::process::Command::new("git")
                .args([
                    "-C",
                    &wt_path.to_string_lossy(),
                    "log",
                    "-1",
                    "--format=%cr",
                    "HEAD",
                ])
                .output()
                .ok()
                .and_then(|o| {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if s.is_empty() { None } else { Some(s) }
                })
                .unwrap_or_else(|| "unknown".to_string());
            format!("{}\t{}", name, date)
        })
        .collect();

    let mut fzf = Command::new("fzf")
        .args(["--prompt", "Worktree: ", "--delimiter=\t"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn fzf — is it installed?")?;

    if let Some(mut stdin) = fzf.stdin.take() {
        stdin.write_all(items.join("\n").as_bytes()).ok();
    }

    let out = fzf.wait_with_output().context("fzf wait failed")?;
    let selected = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if selected.is_empty() {
        bail!("Nothing selected.");
    }
    Ok(selected.split('\t').nth(1).unwrap_or("").trim().to_string())
}

fn build_workspace_json(
    folders: &[WorkspaceFolder],
    peacock_color: Option<&str>,
    extra_settings: Option<&Value>,
) -> Value {
    let folder_arr: Vec<Value> = folders
        .iter()
        .map(|f| {
            let mut obj = serde_json::Map::new();
            if let Some(n) = &f.name {
                obj.insert("name".to_string(), json!(n));
            }
            obj.insert("path".to_string(), json!(f.path));
            Value::Object(obj)
        })
        .collect();

    let mut settings = serde_json::Map::new();
    settings.insert("task.saveBeforeRun".to_string(), json!("never"));

    if let Some(Value::Object(extra)) = extra_settings {
        for (k, v) in extra {
            settings.insert(k.clone(), v.clone());
        }
    }

    if let Some(color) = peacock_color {
        settings.insert("peacock.color".to_string(), json!(color));
        settings.insert(
            "workbench.colorCustomizations".to_string(),
            compute_peacock_color_customizations(color),
        );
    }

    json!({
        "folders": folder_arr,
        "settings": Value::Object(settings),
    })
}

fn write_workspace(path: &Path, ws: &Value) -> Result<()> {
    let json = serde_json::to_string_pretty(ws)?;
    std::fs::write(path, format!("{}\n", json))
        .with_context(|| format!("failed to write workspace file to {}", path.display()))
}

/// If `path` already exists, read its `settings` block and copy any keys that
/// are NOT present in `generated` into the generated settings. This preserves
/// manual VS Code settings across regenerations while ensuring generated keys
/// (peacock color, files.exclude, etc.) always win.
fn merge_existing_settings(path: &Path, generated: Value) -> Value {
    let existing_settings = std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("settings").cloned())
        .and_then(|s| {
            if let Value::Object(m) = s {
                Some(m)
            } else {
                None
            }
        });

    let Some(existing) = existing_settings else {
        return generated;
    };

    let mut out = generated;
    if let Some(Value::Object(generated_settings)) = out.get_mut("settings") {
        for (k, v) in existing {
            generated_settings.entry(k).or_insert(v);
        }
    }
    out
}

fn open_workspace(path: &Path) -> Result<()> {
    Command::new("code").arg(path).status().context(
        "failed to run 'code' — is the VS Code CLI installed?\n\
             In VS Code: Cmd+Shift+P → 'Shell Command: Install code in PATH'",
    )?;
    Ok(())
}

/// Generate a random, visually pleasant Peacock color.
pub fn random_peacock_color() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0x42);
    let hue = (nanos % 360) as f32;
    hsl_to_hex(hue, 0.45, 0.48)
}

fn hsl_to_hex(h: f32, s: f32, l: f32) -> String {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let r = ((r1 + m) * 255.0).round() as u8;
    let g = ((g1 + m) * 255.0).round() as u8;
    let b = ((b1 + m) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-6 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() < 1e-6 {
        60.0 * (((g - b) / d) % 6.0)
    } else if (max - g).abs() < 1e-6 {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (if h < 0.0 { h + 360.0 } else { h }, s, l)
}

/// Derive VS Code workbench.colorCustomizations from a Peacock base color.
pub fn compute_peacock_color_customizations(hex: &str) -> Value {
    let (r, g, b) = match parse_hex_color(hex) {
        Some(c) => c,
        None => return json!({}),
    };
    let (h, s, l) = rgb_to_hsl(r, g, b);
    let bg_l = (l + 0.13).min(1.0_f32);
    let bg = hsl_to_hex(h, s, bg_l);
    let (bg_r, bg_g, bg_b) = parse_hex_color(&bg).unwrap_or((128, 128, 128));
    let luminance = 0.299 * bg_r as f32 + 0.587 * bg_g as f32 + 0.114 * bg_b as f32;
    let (fg, badge_bg) = if luminance > 127.0 {
        ("#15202b", "#ecf2f0")
    } else {
        ("#e7e7e7", "#15202b")
    };
    json!({
        "activityBar.activeBackground": bg,
        "activityBar.background": bg,
        "activityBar.foreground": fg,
        "activityBar.inactiveForeground": format!("{}99", fg),
        "activityBarBadge.background": badge_bg,
        "activityBarBadge.foreground": fg,
        "sash.hoverBorder": bg
    })
}

fn fzf_multiselect(items: &[String], prompt_str: &str) -> Result<Vec<String>> {
    let mut fzf = Command::new("fzf")
        .args(["--multi", "--prompt", prompt_str])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn fzf — is it installed?")?;

    if let Some(mut stdin) = fzf.stdin.take() {
        stdin.write_all(items.join("\n").as_bytes()).ok();
    }

    let out = fzf.wait_with_output().context("fzf wait failed")?;
    if out.stdout.is_empty() {
        return Ok(vec![]);
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

fn fzf_select_tab(items: &[String], prompt_str: &str) -> Result<String> {
    let mut fzf = Command::new("fzf")
        .args(["--prompt", prompt_str, "--delimiter=\t"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn fzf — is it installed?")?;

    if let Some(mut stdin) = fzf.stdin.take() {
        stdin.write_all(items.join("\n").as_bytes()).ok();
    }

    let out = fzf.wait_with_output().context("fzf wait failed")?;
    let selected = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if selected.is_empty() {
        bail!("Nothing selected.");
    }
    Ok(selected)
}

/// Pick a worktree branch for a group of repos, with a "(root checkout)" option first.
/// Returns `None` if the user picks root checkout or if the repos have no worktrees yet.
pub fn pick_group_worktree(
    parent: &std::path::Path,
    repos: &[String],
    prompt: &str,
) -> Result<Option<String>> {
    let repo = match repos.first() {
        Some(r) => r,
        None => return Ok(None),
    };
    let wt_base = parent.join(repo).join(".claude").join("worktrees");

    if !wt_base.exists() {
        eprintln!("  No worktrees found for {} — using root checkout", repo);
        return Ok(None);
    }

    let mut entries: Vec<String> = std::fs::read_dir(&wt_base)
        .context("failed to read worktrees directory")?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    entries.sort();

    if entries.is_empty() {
        eprintln!("  No worktrees found for {} — using root checkout", repo);
        return Ok(None);
    }

    let mut choices = vec!["\t(root checkout)".to_string()];
    for name in &entries {
        let wt_path = wt_base.join(name);
        let date = Command::new("git")
            .args([
                "-C",
                &wt_path.to_string_lossy(),
                "log",
                "-1",
                "--format=%cr",
                "HEAD",
            ])
            .output()
            .ok()
            .and_then(|o| {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if s.is_empty() { None } else { Some(s) }
            })
            .unwrap_or_else(|| "unknown".to_string());
        choices.push(format!("{}\t{}", date, name));
    }

    let raw = fzf_select_tab(&choices, prompt)?;
    let sel = raw.split('\t').nth(1).unwrap_or("").trim().to_string();

    if sel == "(root checkout)" || sel.is_empty() {
        Ok(None)
    } else {
        Ok(Some(sel))
    }
}

pub fn fzf_select(items: &[String], prompt_str: &str) -> Result<String> {
    let mut fzf = Command::new("fzf")
        .args(["--prompt", prompt_str])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn fzf — is it installed?")?;

    if let Some(mut stdin) = fzf.stdin.take() {
        stdin.write_all(items.join("\n").as_bytes()).ok();
    }

    let out = fzf.wait_with_output().context("fzf wait failed")?;
    let selected = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if selected.is_empty() {
        bail!("Nothing selected.");
    }
    Ok(selected)
}

fn prompt(msg: &str) -> Result<String> {
    use std::io::BufRead;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    write!(lock, "{}", msg)?;
    lock.flush()?;
    drop(lock);

    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn make_worktree_dir(parent: &TempDir, repo: &str, branches: &[&str]) {
        for branch in branches {
            let wt = parent
                .path()
                .join(repo)
                .join(".claude")
                .join("worktrees")
                .join(branch);
            fs::create_dir_all(&wt).unwrap();
        }
    }

    // ── rgb_to_hsl / compute_peacock_color_customizations ─────────────────

    #[test]
    fn rgb_to_hsl_red() {
        let (h, s, l) = rgb_to_hsl(255, 0, 0);
        assert!((h - 0.0).abs() < 1.0, "hue should be ~0, got {}", h);
        assert!((s - 1.0).abs() < 0.01);
        assert!((l - 0.5).abs() < 0.01);
    }

    #[test]
    fn rgb_to_hsl_roundtrip() {
        let (h_in, s_in, l_in) = (200.0_f32, 0.45, 0.48);
        let hex = hsl_to_hex(h_in, s_in, l_in);
        let (r, g, b) = parse_hex_color(&hex).unwrap();
        let (h_out, s_out, l_out) = rgb_to_hsl(r, g, b);
        assert!(
            (h_out - h_in).abs() < 2.0,
            "hue drift: {} vs {}",
            h_out,
            h_in
        );
        assert!(
            (s_out - s_in).abs() < 0.02,
            "sat drift: {} vs {}",
            s_out,
            s_in
        );
        assert!(
            (l_out - l_in).abs() < 0.02,
            "lit drift: {} vs {}",
            l_out,
            l_in
        );
    }

    #[test]
    fn peacock_colors_known_pair() {
        let colors = compute_peacock_color_customizations("#a0927a");
        let bg = colors["activityBar.background"].as_str().unwrap();
        let (_, _, l_base) = rgb_to_hsl(160, 146, 122);
        let (r, g, b) = parse_hex_color(bg).unwrap();
        let (_, _, l_bg) = rgb_to_hsl(r, g, b);
        assert!(
            l_bg > l_base,
            "bg should be lighter than base: {} vs {}",
            l_bg,
            l_base
        );
        let fg = colors["activityBar.foreground"].as_str().unwrap();
        assert!(fg.starts_with('#'));
    }

    #[test]
    fn peacock_colors_dark_base_gets_light_fg() {
        let colors = compute_peacock_color_customizations("#0059e6");
        let fg = colors["activityBar.foreground"].as_str().unwrap();
        let (r, g, b) = parse_hex_color(fg).unwrap();
        let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        assert!(
            luminance > 127.0,
            "expected light fg on dark bg, got {}",
            fg
        );
    }

    #[test]
    fn peacock_colors_all_required_keys_present() {
        let colors = compute_peacock_color_customizations("#a0927a");
        for key in &[
            "activityBar.activeBackground",
            "activityBar.background",
            "activityBar.foreground",
            "activityBar.inactiveForeground",
            "activityBarBadge.background",
            "activityBarBadge.foreground",
            "sash.hoverBorder",
        ] {
            assert!(colors.get(key).is_some(), "missing key: {}", key);
        }
    }

    #[test]
    fn peacock_colors_invalid_hex_returns_empty() {
        let colors = compute_peacock_color_customizations("notacolor");
        assert_eq!(colors, json!({}));
    }

    #[test]
    fn parse_hex_color_valid() {
        assert_eq!(parse_hex_color("#ff0000"), Some((255, 0, 0)));
        assert_eq!(parse_hex_color("#00ff00"), Some((0, 255, 0)));
        assert_eq!(parse_hex_color("#0059e6"), Some((0, 89, 230)));
    }

    #[test]
    fn parse_hex_color_without_hash() {
        assert_eq!(parse_hex_color("a0927a"), Some((160, 146, 122)));
    }

    #[test]
    fn parse_hex_color_invalid() {
        assert!(parse_hex_color("#gggggg").is_none());
        assert!(parse_hex_color("#fff").is_none());
        assert!(parse_hex_color("").is_none());
    }

    #[test]
    fn color_swatch_contains_ansi_bg_code() {
        let swatch = color_swatch("#ff0000");
        assert!(
            swatch.contains("\x1b[48;2;255;0;0m"),
            "expected true-color bg code"
        );
        assert!(swatch.ends_with("\x1b[0m"), "expected reset at end");
    }

    #[test]
    fn color_swatch_bad_hex_returns_spaces() {
        let swatch = color_swatch("bad");
        assert_eq!(swatch, "     ");
        assert!(!swatch.contains('\x1b'));
    }

    #[test]
    fn strip_ansi_removes_escape_codes() {
        let input = "\x1b[48;2;255;0;0m     \x1b[0m #ff0000  red";
        let plain = strip_ansi(input);
        assert!(!plain.contains('\x1b'));
        assert!(plain.contains("#ff0000"));
        assert!(plain.contains("red"));
    }

    #[test]
    fn strip_ansi_passthrough_plain_text() {
        let input = "hello world";
        assert_eq!(strip_ansi(input), "hello world");
    }

    #[test]
    fn hex_extractable_from_swatch_line() {
        let hex = "#0059e6";
        let line = format!("{} {}  blue", color_swatch(hex), hex);
        let plain = strip_ansi(&line);
        let found = plain
            .split_whitespace()
            .find(|w| w.starts_with('#') && w.len() == 7);
        assert_eq!(found, Some("#0059e6"));
    }

    #[test]
    fn build_json_folders_with_and_without_name() {
        let folders = vec![
            WorkspaceFolder {
                name: Some("my-api".to_string()),
                path: "my-api".to_string(),
            },
            WorkspaceFolder {
                name: None,
                path: "docs".to_string(),
            },
        ];
        let ws = build_workspace_json(&folders, None, None);
        let arr = ws["folders"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["name"], "my-api");
        assert_eq!(arr[0]["path"], "my-api");
        assert!(
            arr[1].get("name").is_none(),
            "name key must be absent when not set"
        );
        assert_eq!(arr[1]["path"], "docs");
    }

    #[test]
    fn build_json_peacock_color_emitted() {
        let ws = build_workspace_json(&[], Some("#a0927a"), None);
        assert_eq!(ws["settings"]["peacock.color"], "#a0927a");
    }

    #[test]
    fn build_json_no_peacock_key_when_color_absent() {
        let ws = build_workspace_json(&[], None, None);
        assert!(ws["settings"].get("peacock.color").is_none());
    }

    #[test]
    fn build_json_extra_settings_merged() {
        let extra = serde_json::json!({
            "files.exclude": { "**/.git": true },
            "workbench.colorCustomizations": { "activityBar.background": "#b5aa98" }
        });
        let ws = build_workspace_json(&[], None, Some(&extra));
        assert_eq!(ws["settings"]["files.exclude"]["**/.git"], true);
        assert_eq!(
            ws["settings"]["workbench.colorCustomizations"]["activityBar.background"],
            "#b5aa98"
        );
    }

    #[test]
    fn build_json_always_includes_defaults() {
        let ws = build_workspace_json(&[], None, None);
        assert_eq!(ws["settings"]["task.saveBeforeRun"], "never");
    }

    #[test]
    fn build_json_extra_settings_can_add_enterprise_uri() {
        let extra = serde_json::json!({
            "github-enterprise.uri": "https://github.example.com"
        });
        let ws = build_workspace_json(&[], None, Some(&extra));
        assert_eq!(
            ws["settings"]["github-enterprise.uri"],
            "https://github.example.com"
        );
    }

    #[test]
    fn vscode_preset_simple_folders_round_trip() {
        let json = r#"{"name":"simple","folders":["demo-ui","demo-api"]}"#;
        let preset: VscodePreset = serde_json::from_str(json).unwrap();
        assert_eq!(preset.name, "simple");
        assert_eq!(preset.folders.len(), 2);
        assert_eq!(preset.folders[0].key(), "demo-ui");
        assert_eq!(preset.folders[1].key(), "demo-api");
        assert!(preset.folders[0].explicit_branch().is_none());
    }

    #[test]
    fn vscode_preset_with_branch_folder() {
        let json = r#"{"name":"mixed","folders":[{"repo":"demo-ui","branch":"feat_frontend"},{"repo":"demo-api","branch":"feat_backend"},"docs"]}"#;
        let preset: VscodePreset = serde_json::from_str(json).unwrap();
        assert_eq!(preset.folders.len(), 3);
        assert_eq!(preset.folders[0].key(), "demo-ui");
        assert_eq!(preset.folders[0].explicit_branch(), Some("feat_frontend"));
        assert_eq!(preset.folders[1].key(), "demo-api");
        assert_eq!(preset.folders[1].explicit_branch(), Some("feat_backend"));
        assert_eq!(preset.folders[2].key(), "docs");
        assert!(preset.folders[2].explicit_branch().is_none());
    }

    #[test]
    fn random_peacock_color_is_valid_hex() {
        let color = random_peacock_color();
        assert!(color.starts_with('#'), "expected #rrggbb, got {}", color);
        assert_eq!(color.len(), 7, "expected 7 chars, got {}", color);
        assert!(
            color[1..].chars().all(|c| c.is_ascii_hexdigit()),
            "non-hex chars in {}",
            color
        );
    }

    #[test]
    fn random_peacock_color_varies() {
        let a = random_peacock_color();
        let b = random_peacock_color();
        assert_eq!(a.len(), 7);
        assert_eq!(b.len(), 7);
    }

    #[test]
    fn parse_group_args_valid() {
        let raw = vec![
            "ui=feat_frontend".to_string(),
            "backend=feat_multi-tenant".to_string(),
        ];
        let pairs = parse_group_args(&raw).unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("ui".to_string(), "feat_frontend".to_string()));
        assert_eq!(
            pairs[1],
            ("backend".to_string(), "feat_multi-tenant".to_string())
        );
    }

    #[test]
    fn parse_group_args_missing_equals_errors() {
        let raw = vec!["ui-feat_frontend".to_string()];
        assert!(parse_group_args(&raw).is_err());
        let msg = parse_group_args(&raw).unwrap_err().to_string();
        assert!(
            msg.contains("NAME=BRANCH") || msg.contains("name=branch") || msg.contains("form"),
            "unexpected: {}",
            msg
        );
    }

    #[test]
    fn parse_group_args_empty_name_errors() {
        let raw = vec!["=feat_frontend".to_string()];
        assert!(parse_group_args(&raw).is_err());
    }

    #[test]
    fn parse_group_args_empty_branch_errors() {
        let raw = vec!["ui=".to_string()];
        assert!(parse_group_args(&raw).is_err());
    }

    #[test]
    fn parse_group_args_empty_input_ok() {
        let pairs = parse_group_args(&[]).unwrap();
        assert!(pairs.is_empty());
    }

    fn make_repo_branch_map(
        groups: &[(&str, &[&str])],
        group_args: &[(&str, &str)],
    ) -> std::collections::HashMap<String, String> {
        let vscode_groups: Vec<VscodeGroup> = groups
            .iter()
            .map(|(name, repos)| VscodeGroup {
                name: name.to_string(),
                repos: repos.iter().map(|r| r.to_string()).collect(),
            })
            .collect();
        let args: Vec<(String, String)> = group_args
            .iter()
            .map(|(n, b)| (n.to_string(), b.to_string()))
            .collect();

        vscode_groups
            .iter()
            .filter_map(|g| args.iter().find(|(n, _)| n == &g.name).map(|(_, b)| (g, b)))
            .flat_map(|(g, b)| g.repos.iter().map(move |r| (r.clone(), b.clone())))
            .collect()
    }

    #[test]
    fn group_branch_applies_to_group_repos_only() {
        let map = make_repo_branch_map(
            &[
                ("ui", &["demo-ui"]),
                ("backend", &["demo-api"]),
            ],
            &[("ui", "feat_frontend")],
        );
        assert_eq!(map.get("demo-ui").map(String::as_str), Some("feat_frontend"));
        assert!(!map.contains_key("demo-api"), "backend should not be set");
    }

    #[test]
    fn multiple_groups_independent() {
        let map = make_repo_branch_map(
            &[
                ("ui", &["demo-ui"]),
                ("backend", &["demo-api"]),
            ],
            &[("ui", "feat_frontend"), ("backend", "feat_backend")],
        );
        assert_eq!(map.get("demo-ui").map(String::as_str), Some("feat_frontend"));
        assert_eq!(
            map.get("demo-api").map(String::as_str),
            Some("feat_backend")
        );
    }

    #[test]
    fn unset_group_repos_have_no_entry() {
        let map = make_repo_branch_map(
            &[("ui", &["demo-ui"]), ("backend", &["demo-api"])],
            &[("backend", "feat_backend")],
        );
        assert!(!map.contains_key("demo-ui"));
        assert_eq!(map.get("demo-api").map(String::as_str), Some("feat_backend"));
    }

    #[test]
    fn hsl_to_hex_red() {
        let hex = hsl_to_hex(0.0, 1.0, 0.5);
        assert_eq!(hex, "#ff0000");
    }

    #[test]
    fn hsl_to_hex_green() {
        let hex = hsl_to_hex(120.0, 1.0, 0.5);
        assert_eq!(hex, "#00ff00");
    }

    #[test]
    fn hsl_to_hex_blue() {
        let hex = hsl_to_hex(240.0, 1.0, 0.5);
        assert_eq!(hex, "#0000ff");
    }

    #[test]
    fn hsl_to_hex_white() {
        let hex = hsl_to_hex(0.0, 0.0, 1.0);
        assert_eq!(hex, "#ffffff");
    }

    #[test]
    fn hsl_to_hex_black() {
        let hex = hsl_to_hex(0.0, 0.0, 0.0);
        assert_eq!(hex, "#000000");
    }

    #[test]
    fn vscode_config_full_deserialize() {
        let json = r##"{
            "output_file": "test.code-workspace",
            "peacock_color": "#a0927a",
            "extra_folders": [{"name": "docs", "path": "docs"}, {"path": "infra"}],
            "presets": [{"name": "simple", "folders": ["demo-ui", "demo-api"]}]
        }"##;
        let cfg: VscodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.output_file.as_deref(), Some("test.code-workspace"));
        assert_eq!(cfg.peacock_color.as_deref(), Some("#a0927a"));
        assert_eq!(cfg.extra_folders.len(), 2);
        assert_eq!(cfg.extra_folders[0].name.as_deref(), Some("docs"));
        assert!(cfg.extra_folders[1].name.is_none());
        assert_eq!(cfg.presets.len(), 1);
        assert_eq!(cfg.presets[0].name, "simple");
    }

    #[test]
    fn vscode_config_defaults_when_empty() {
        let cfg: VscodeConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.output_file.is_none());
        assert!(cfg.peacock_color.is_none());
        assert!(cfg.presets.is_empty());
        assert!(cfg.extra_folders.is_empty());
    }

    #[test]
    fn pick_worktree_ff_auto_selects_single_entry() {
        let tmp = tempfile::tempdir().unwrap();
        make_worktree_dir(&tmp, "my-svc", &["feat_my-branch"]);

        let result = pick_worktree_branch(tmp.path(), &["my-svc".to_string()], true).unwrap();
        assert_eq!(result, "feat_my-branch");
    }

    #[test]
    #[ignore = "requires interactive TTY — fzf blocks in CI"]
    fn pick_worktree_ff_false_does_not_auto_select() {
        let tmp = tempfile::tempdir().unwrap();
        make_worktree_dir(&tmp, "my-svc", &["feat_only"]);

        let result = pick_worktree_branch(tmp.path(), &["my-svc".to_string()], false);
        assert!(result.is_err());
    }

    #[test]
    fn pick_worktree_no_worktrees_dir_errors() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("my-svc")).unwrap();

        let result = pick_worktree_branch(tmp.path(), &["my-svc".to_string()], true);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("No worktrees found"), "unexpected: {}", msg);
    }

    #[test]
    fn pick_worktree_empty_worktrees_dir_errors() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("my-svc").join(".claude").join("worktrees")).unwrap();

        let result = pick_worktree_branch(tmp.path(), &["my-svc".to_string()], true);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("No worktree entries"), "unexpected: {}", msg);
    }

    #[test]
    fn pick_worktree_no_services_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let result = pick_worktree_branch(tmp.path(), &[], true);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("No services"), "unexpected: {}", msg);
    }

    #[test]
    fn save_preset_appends_to_existing_presets() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let settings_path = tmp.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"project":{"name":"Test"},"vscode":{"presets":[{"name":"a","folders":["x"]}]}}"#,
        )
        .unwrap();

        unsafe { std::env::set_var("PENV_REPO_ROOT", tmp.path()) };
        let preset = VscodePreset {
            name: "b".to_string(),
            folders: vec![FolderKey::Simple("y".to_string())],
            peacock_color: None,
        };
        save_preset_to_settings(&preset).unwrap();
        unsafe { std::env::remove_var("PENV_REPO_ROOT") };

        let raw = fs::read_to_string(&settings_path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let presets = val["vscode"]["presets"].as_array().unwrap();
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[1]["name"], "b");
        assert_eq!(presets[1]["folders"][0], "y");
    }

    #[test]
    fn save_preset_creates_vscode_block_if_missing() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let settings_path = tmp.path().join("settings.json");
        fs::write(&settings_path, r#"{"project":{"name":"Test"}}"#).unwrap();

        unsafe { std::env::set_var("PENV_REPO_ROOT", tmp.path()) };
        let preset = VscodePreset {
            name: "simple".to_string(),
            folders: vec![FolderKey::Simple("demo-ui".to_string())],
            peacock_color: None,
        };
        save_preset_to_settings(&preset).unwrap();
        unsafe { std::env::remove_var("PENV_REPO_ROOT") };

        let raw = fs::read_to_string(&settings_path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["vscode"]["presets"][0]["name"], "simple");
    }
}
