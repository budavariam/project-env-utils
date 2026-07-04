/// `penv morning-check` — interactive start-of-day sync check.
///
/// Checks every tracked workspace and service in .state/active.json.
/// With secret backend: compare local .env against remote, offer diff/push/pull/skip.
/// Without backend: confirm preset, offer reload from profile.
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::backend::SecretBackend;
use crate::cmd::sync::unified_diff;
use crate::config::{
    backups_dir, fallback_profile_path, repo_root, Settings,
};
use crate::env_file::{read_file, write_file};
use crate::state::{all_tracked_services, all_tracked_workspaces, State};

// ── Backup session ─────────────────────────────────────────────────────────────

struct BackupSession {
    dir: Option<PathBuf>,
    log: Vec<String>,
    timestamp: String,
}

impl BackupSession {
    fn new(timestamp: &str) -> Self {
        BackupSession {
            dir: None,
            log: vec![format!("morning_check — {}", timestamp), "=".repeat(60)],
            timestamp: timestamp.to_string(),
        }
    }

    fn dir_path(&mut self) -> &Path {
        if self.dir.is_none() {
            let d = backups_dir().join(&self.timestamp);
            let _ = std::fs::create_dir_all(&d);
            self.dir = Some(d);
        }
        self.dir.as_ref().unwrap()
    }

    fn save(&mut self, label: &str, local: Option<&str>, remote: Option<&str>) {
        let d = self.dir_path().to_path_buf();
        if let Some(l) = local {
            let _ = std::fs::write(d.join(format!("{}.local.env", label)), l);
        }
        if let Some(r) = remote {
            let _ = std::fs::write(d.join(format!("{}.remote.env", label)), r);
        }
    }

    fn log(&mut self, msg: &str) {
        self.log.push(msg.to_string());
    }

    fn flush(&self) {
        if let Some(dir) = &self.dir {
            let content = self.log.join("\n") + "\n";
            let _ = std::fs::write(dir.join("sync.log"), content);
            // Show relative path if inside repo root
            let rel = dir
                .strip_prefix(repo_root())
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| dir.display().to_string());
            println!("\n  Backups saved → {}/", rel);
        }
    }
}

// ── User interaction helpers ───────────────────────────────────────────────────

fn ask(question: &str, default: &str) -> String {
    print!("  {} [{}]: ", question, default);
    io::stdout().flush().ok();
    let stdin = io::stdin();
    match stdin.lock().lines().next() {
        Some(Ok(l)) => {
            let val = l.trim().to_string();
            if val.is_empty() {
                default.to_string()
            } else {
                val
            }
        }
        _ => {
            println!();
            std::process::exit(0);
        }
    }
}

fn menu(options: &[(&str, &str)], default_key: &str) -> String {
    for (key, label) in options {
        let hint = if *key == default_key {
            "  ← default (Enter)"
        } else {
            ""
        };
        println!("    [{}] {}{}", key, label, hint);
    }
    loop {
        print!("  Choice [{}]: ", default_key);
        io::stdout().flush().ok();
        let stdin = io::stdin();
        let val = match stdin.lock().lines().next() {
            Some(Ok(l)) => l.trim().to_lowercase(),
            _ => {
                println!();
                std::process::exit(0);
            }
        };
        if val.is_empty() {
            return default_key.to_string();
        }
        if options.iter().any(|(k, _)| *k == val) {
            return val;
        }
        let keys: Vec<&str> = options.iter().map(|(k, _)| *k).collect();
        println!("  Please enter one of: {}", keys.join(", "));
    }
}

fn show_diff(local: &str, remote: &str, from_label: &str, to_label: &str) {
    let diff = unified_diff(remote, local, from_label, to_label, 60);
    if diff.lines().all(|l| {
        l.starts_with(' ') || l.starts_with('-') || l.starts_with('+') || l.starts_with('@')
    }) {
        // Check if it's actually empty (only header lines)
        let changes: Vec<_> = diff
            .lines()
            .filter(|l| l.starts_with('+') || l.starts_with('-'))
            .collect();
        if changes.is_empty() {
            println!("    (files are identical byte-for-byte)");
            return;
        }
    }
    for line in diff.lines() {
        let ch = line.chars().next().unwrap_or(' ');
        let prefix = match ch {
            '+' => "  + ",
            '-' => "  - ",
            '@' => "  @ ",
            _ => "    ",
        };
        println!("{}{}", prefix, line.trim_end_matches('\n'));
    }
}

// ── Core handler ───────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn handle(
    label: &str,
    service: &str,
    local_path: &Path,
    preset: &str,
    backend: Option<&dyn SecretBackend>,
    backup: &mut BackupSession,
    save_state: &dyn Fn(&str),
    remove_entry: &dyn Fn(),
    settings: &Settings,
) {
    let local = read_file(local_path);

    // ── No backend ─────────────────────────────────────────────────────────────
    let Some(b) = backend else {
        if local.is_none() {
            println!("\n  {} [{}]: local .env missing", label, preset);
            let profile = fallback_profile_path(service, preset);
            if profile.exists() {
                let choice = menu(
                    &[
                        (
                            "y",
                            &format!("Load from profile env/{}/{}.env", service, preset),
                        ),
                        ("r", "Remove this entry from tracking"),
                        ("s", "Skip"),
                    ],
                    "y",
                );
                if choice == "y" {
                    if let Ok(content) = std::fs::read_to_string(&profile) {
                        let _ = write_file(local_path, &content);
                        save_state(preset);
                        backup.log(&format!(
                            "\n[{}] loaded from local profile (no backend)",
                            label
                        ));
                        println!("    Loaded from profile.");
                    }
                } else if choice == "r" {
                    remove_entry();
                    backup.log(&format!(
                        "\n[{}] removed from tracking (local missing, no profile)",
                        label
                    ));
                    println!("    Removed from tracking.");
                }
            } else {
                println!("    No profile at env/{}/{}.env.", service, preset);
                let choice = menu(
                    &[
                        ("r", "Remove this entry from tracking"),
                        ("s", "Skip (will appear again next run)"),
                    ],
                    "s",
                );
                if choice == "r" {
                    remove_entry();
                    backup.log(&format!(
                        "\n[{}] removed from tracking (nothing to load)",
                        label
                    ));
                    println!("    Removed from tracking.");
                } else {
                    backup.log(&format!(
                        "\n[{}] skipped (local .env missing, no profile, no backend)",
                        label
                    ));
                }
            }
        } else {
            let preset_local_path = settings.project.preset_local_path(service, preset);
            let preset_local = read_file(&preset_local_path);

            if let Some(ref pl) = preset_local {
                let local_str = local.as_deref().unwrap_or("");
                if local_str != pl.as_str() {
                    println!(
                        "\n  {} [{}]: .env differs from {}",
                        label, preset, settings.project.preset_local_rel(service, preset)
                    );
                    backup.save(label, local.as_deref(), Some(pl));
                    let reload_label = format!("Reload from {}", settings.project.preset_local_rel(service, preset));
                    let write_label = format!("Write current .env → {}", settings.project.preset_local_rel(service, preset));
                    let vault_label_opt = backend.map(|b| format!("Push current .env → {}", b.label()));
                    let mut opt_strs: Vec<(&str, &str)> = vec![
                        ("d", "Show diff"),
                        ("r", reload_label.as_str()),
                        ("w", write_label.as_str()),
                    ];
                    if let Some(ref vl) = vault_label_opt {
                        opt_strs.push(("v", vl.as_str()));
                    }
                    opt_strs.push(("k", "Keep current .env as-is"));
                    opt_strs.push(("s", "Skip"));
                    let mut choice = menu(&opt_strs, "r");
                    while choice == "d" {
                        show_diff(
                            local_str,
                            pl,
                            &format!("current {service}/.env"),
                            &format!("local/{service}.{preset}.env"),
                        );
                        choice = menu(&opt_strs, "r");
                    }
                    if choice == "r" {
                        let _ = write_file(local_path, pl);
                        save_state(preset);
                        backup.log(&format!(
                            "\n[{}] reloaded from {}",
                            label, settings.project.preset_local_rel(service, preset)
                        ));
                        println!("    Reloaded.");
                    } else if choice == "w" {
                        let _ = std::fs::write(&preset_local_path, local_str);
                        save_state(preset);
                        backup.log(&format!(
                            "\n[{}] wrote current .env → {}",
                            label, settings.project.preset_local_rel(service, preset)
                        ));
                        println!("    Saved to {}.", settings.project.preset_local_rel(service, preset));
                    } else if choice == "v" {
                        if let Some(b) = backend {
                            push_with_bucket(b, service, preset, local_str);
                            save_state(preset);
                            backup.log(&format!(
                                "\n[{}] pushed current .env → {}",
                                label, b.label()
                            ));
                            println!("    Pushed to {}.", b.label());
                        }
                    } else {
                        save_state(preset);
                        backup.log(&format!(
                            "\n[{}] kept current .env (differs from preset file)",
                            label
                        ));
                        println!("    Kept as-is.");
                    }
                    return;
                }
            }

            let new_preset = ask(
                &format!("{}: secret backend unavailable. Keep preset", label),
                preset,
            );
            save_state(&new_preset);
            if new_preset != preset {
                let profile = fallback_profile_path(service, &new_preset);
                if profile.exists() {
                    backup.save(label, local.as_deref(), None);
                    if let Ok(content) = std::fs::read_to_string(&profile) {
                        let _ = write_file(local_path, &content);
                    }
                    backup.log(&format!(
                        "\n[{}] switched {} → {} from local profile",
                        label, preset, new_preset
                    ));
                    println!("    Reloaded from profile for preset '{}'.", new_preset);
                } else {
                    println!(
                        "    No profile found for '{}' — local .env unchanged.",
                        new_preset
                    );
                    backup.log(&format!(
                        "\n[{}] wanted '{}' but no profile found, unchanged",
                        label, new_preset
                    ));
                }
            } else {
                backup.log(&format!(
                    "\n[{}] confirmed preset={} (no backend, local .env unchanged)",
                    label, preset
                ));
                println!(
                    "    {} [{}]: confirmed (backend unavailable)",
                    label, preset
                );
            }
        }
        return;
    };

    // ── Backend available ──────────────────────────────────────────────────────
    let remote = b.fetch(service, preset);

    match (local.as_deref(), remote.as_deref()) {
        (None, None) => {
            println!(
                "  {} [{}]: .env missing locally and in {}",
                label,
                preset,
                b.label()
            );
            let choice = menu(
                &[("r", "Remove this entry from tracking"), ("s", "Skip")],
                "s",
            );
            if choice == "r" {
                remove_entry();
                backup.log(&format!(
                    "\n[{}] removed from tracking (both missing)",
                    label
                ));
            } else {
                backup.log(&format!("\n[{}] both missing, skipped", label));
            }
        }
        (Some(loc), None) => {
            println!(
                "\n  {} [{}]: local .env exists but not in {} ({})",
                label,
                preset,
                b.label(),
                b.key_display(service, preset)
            );
            backup.save(label, Some(loc), None);
            let choice = menu(
                &[("l", &format!("Push local → {}", b.label())), ("s", "Skip")],
                "l",
            );
            if choice == "l" {
                push_with_bucket(b, service, preset, loc);
                save_state(preset);
                backup.log(&format!(
                    "\n[{}] pushed local → {} ({} was missing)",
                    label,
                    b.label(),
                    b.label()
                ));
                println!("    Pushed.");
            } else {
                backup.log(&format!(
                    "\n[{}] skipped (local exists, {} missing)",
                    label,
                    b.label()
                ));
            }
        }
        (None, Some(rem)) => {
            println!(
                "\n  {} [{}]: {} has {} but local .env is missing",
                label,
                preset,
                b.label(),
                b.key_display(service, preset)
            );
            backup.save(label, None, Some(rem));
            let choice = menu(
                &[
                    ("r", "Fetch from backend → create local .env"),
                    ("s", "Skip"),
                ],
                "r",
            );
            if choice == "r" {
                let _ = write_file(local_path, rem);
                save_state(preset);
                backup.log(&format!(
                    "\n[{}] fetched {} → created local .env",
                    label,
                    b.label()
                ));
                println!("    Written.");
            } else {
                backup.log(&format!("\n[{}] skipped (local missing)", label));
            }
        }
        (Some(loc), Some(rem)) => {
            if loc == rem {
                save_state(preset);
                println!("  {} [{}]: in sync ✓", label, preset);
                backup.log(&format!("\n[{}] in sync", label));
                return;
            }

            println!("\n  {} [{}]: differs from {}", label, preset, b.label());
            backup.save(label, Some(loc), Some(rem));

            let opts: &[(&str, &str)] = &[
                ("d", "Show diff"),
                ("l", &format!("Keep local  — push local → {}", b.label())),
                ("r", &format!("Use {}  — overwrite local", b.label())),
                ("b", "Keep both   — save backup, change nothing"),
                ("s", "Skip"),
            ];
            let mut choice = menu(opts, "s");
            while choice == "d" {
                show_diff(
                    loc,
                    rem,
                    &format!("{}/{}", b.label().to_lowercase(), b.key_display(service, preset)),
                    &format!("local/{service}/.env"),
                );
                choice = menu(opts, "s");
            }

            if choice == "l" {
                push_with_bucket(b, service, preset, loc);
                save_state(preset);
                backup.log(&format!("\n[{}] kept local, pushed → {}", label, b.label()));
                println!("    Pushed local → {}.", b.label());
            } else if choice == "r" {
                let _ = write_file(local_path, rem);
                save_state(preset);
                backup.log(&format!(
                    "\n[{}] used {}, overwrote local",
                    label,
                    b.label()
                ));
                println!("    Local .env replaced from {}.", b.label());
            } else if choice == "b" {
                backup.log(&format!("\n[{}] kept both, backup saved, no change", label));
                println!("    Both versions saved to backup. Nothing changed.");
            } else {
                backup.log(&format!("\n[{}] skipped", label));
                println!("    Skipped.");
            }
        }
    }
}

/// Push to backend.
fn push_with_bucket(backend: &dyn SecretBackend, service: &str, preset: &str, content: &str) {
    backend.push(service, preset, content);
    backend.post_push(service, preset);
}

// ── Interactive startup sync check ────────────────────────────────────────────

/// Interactive env sync check run at dev session startup.
///
/// For each (service, workspace) pair:
/// - In sync → print "✓", continue.
/// - Drift detected → prompt: [d] show diff  [p] pull  [s] skip
/// - Local missing → prompt: [p] pull  [s] skip  (default: pull)
/// - Local not in backend → prompt: [push]  [s] skip
/// - No backend → falls back to quiet one-line status (no prompt).
pub fn startup_sync_check(
    services: &[(&str, Option<&str>)],
    preset: &str,
    backend: Option<&dyn SecretBackend>,
    settings: &Settings,
) {
    println!("Checking env sync for preset '{}':", preset);
    for (service, workspace) in services {
        let label = workspace
            .and_then(|ws| std::path::Path::new(ws).file_name().and_then(|n| n.to_str()))
            .map(|name| format!("{}/{}", service, name))
            .unwrap_or_else(|| service.to_string());
        let local_path = settings.project.service_env_path(service, *workspace);

        let Some(b) = backend else {
            check_one_quiet(&label, service, &local_path, preset, backend, settings);
            continue;
        };

        let local = read_file(&local_path);
        let remote = b.fetch(service, preset);

        match (local.as_deref(), remote.as_deref()) {
            // ── Already in sync ──────────────────────────────────────────────
            (Some(loc), Some(rem)) if loc == rem => {
                println!("  {} [{}]: in sync ✓", label, preset);
            }

            // ── Remote differs from local ────────────────────────────────────
            (Some(loc), Some(rem)) => {
                let cache_path = settings.project.preset_local_path(service, preset);
                let local_is_newer = is_service_env_newer(&local_path, &cache_path);
                let direction = if local_is_newer {
                    "(local .env is newer — your changes likely)"
                } else {
                    &format!("({} was updated — backend changes likely)", b.label())
                };
                println!(
                    "\n  {} [{}]: differs from {}  {}",
                    label, preset, b.label(), direction
                );
                let default = if local_is_newer { "push" } else { "p" };
                let from_label = format!("{}/{}", b.label().to_lowercase(), b.key_display(service, preset));
                let to_label = format!("local/{}", label);
                let push_desc = format!("push local → {}  (your changes → backend)", b.label());
                let pull_desc = format!("pull from {}  (overwrite local)", b.label());
                let diff_desc = format!("show diff  ({} → local)", b.label());
                let opts: &[(&str, &str)] = &[
                    ("d", &diff_desc),
                    ("push", &push_desc),
                    ("p", &pull_desc),
                    ("s", "skip  (keep local as-is)"),
                ];
                let mut choice = menu(opts, default);
                while choice == "d" {
                    show_diff(loc, rem, &from_label, &to_label);
                    choice = menu(opts, default);
                }
                if choice == "p" {
                    crate::backup::backup_env_before_write(
                        &label, &local_path, "startup-sync-pull",
                        &format!("user chose pull from {}", b.label()),
                    );
                    if let Err(e) = write_file(&local_path, rem) {
                        eprintln!("    error writing .env: {}", e);
                    } else {
                        save_preset_state(service, *workspace, preset);
                        println!("    Pulled. Local .env updated from {}.", b.label());
                    }
                } else if choice == "push" {
                    crate::backup::log_backend_push(
                        &label, "startup-sync-push",
                        &format!("user chose push to {}", b.label()),
                    );
                    push_with_bucket(b, service, preset, loc);
                    save_preset_state(service, *workspace, preset);
                    println!("    Pushed to {}.", b.label());
                } else {
                    println!("    Skipped.");
                }
            }

            // ── Local missing, remote exists ─────────────────────────────────
            (None, Some(rem)) => {
                println!(
                    "\n  {} [{}]: .env missing — found in {}",
                    label, preset, b.label()
                );
                let opts: &[(&str, &str)] = &[
                    ("p", &format!("pull from {}  (create local .env)", b.label())),
                    ("s", "skip"),
                ];
                let choice = menu(opts, "p");
                if choice == "p" {
                    if let Some(parent) = local_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    crate::backup::backup_env_before_write(
                        &label, &local_path, "startup-sync-pull",
                        &format!("created from {} (was missing locally)", b.label()),
                    );
                    if let Err(e) = write_file(&local_path, rem) {
                        eprintln!("    error writing .env: {}", e);
                    } else {
                        save_preset_state(service, *workspace, preset);
                        println!("    Pulled. Local .env created.");
                    }
                } else {
                    println!("    Skipped.");
                }
            }

            // ── Local exists, not yet in backend ─────────────────────────────
            (Some(loc), None) => {
                println!(
                    "\n  {} [{}]: local .env not in {} yet",
                    label, preset, b.label()
                );
                let opts: &[(&str, &str)] = &[
                    ("push", &format!("push to {}", b.label())),
                    ("s", "skip"),
                ];
                let choice = menu(opts, "s");
                if choice == "push" {
                    crate::backup::log_backend_push(
                        &label, "startup-sync-push",
                        &format!("user pushed local .env to {} (was missing in backend)", b.label()),
                    );
                    push_with_bucket(b, service, preset, loc);
                    println!("    Pushed to {}.", b.label());
                } else {
                    println!("    Skipped.");
                }
            }

            // ── Nothing anywhere ─────────────────────────────────────────────
            (None, None) => {
                println!(
                    "  {} [{}]: .env missing locally and in {}",
                    label, preset, b.label()
                );
            }
        }
    }
}

/// Returns true if the service .env is newer than the penv local cache.
/// If the cache doesn't exist, the service .env counts as newer (user may have
/// made changes that were never captured).
fn is_service_env_newer(service_env: &Path, cache_path: &Path) -> bool {
    let env_mtime = std::fs::metadata(service_env).and_then(|m| m.modified()).ok();
    let cache_mtime = std::fs::metadata(cache_path).and_then(|m| m.modified()).ok();
    match (env_mtime, cache_mtime) {
        (Some(e), Some(c)) => e > c,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Update preset state for a service or workspace after a pull.
fn save_preset_state(service: &str, workspace: Option<&str>, preset: &str) {
    if let Some(ws) = workspace {
        let _ = crate::state::set_workspace_preset(ws, preset);
    } else {
        let _ = crate::state::set_service_preset(service, preset);
    }
}

// ── Non-interactive quiet status (no-backend fallback) ────────────────────────

fn check_one_quiet(
    label: &str,
    service: &str,
    local_path: &std::path::Path,
    preset: &str,
    backend: Option<&dyn SecretBackend>,
    settings: &Settings,
) {
    let local = read_file(local_path);

    if let Some(b) = backend {
        let remote = b.fetch(service, preset);
        match (local.as_deref(), remote.as_deref()) {
            (None, _) => println!(
                "  {} [{}]: .env missing locally — run reload_env",
                label, preset
            ),
            (Some(loc), Some(rem)) if loc == rem => {
                println!("  {} [{}]: in sync ✓", label, preset)
            }
            (Some(_), Some(_)) => println!(
                "  {} [{}]: differs from {} — run reload_env to update",
                label,
                preset,
                b.label()
            ),
            (Some(_), None) => println!(
                "  {} [{}]: not in {} yet — run morning-check to push",
                label,
                preset,
                b.label()
            ),
        }
    } else {
        let preset_local = settings.project.preset_local_path(service, preset);
        let reference = if preset_local.exists() {
            read_file(&preset_local)
        } else {
            read_file(&fallback_profile_path(service, preset))
        };

        match (local.as_deref(), reference.as_deref()) {
            (None, _) => println!("  {} [{}]: .env missing — run reload_env", label, preset),
            (Some(loc), Some(r)) if loc == r => {
                println!("  {} [{}]: matches reference ✓", label, preset)
            }
            (Some(loc), Some(r)) => {
                let ref_label = if preset_local.exists() {
                    settings.project.preset_local_rel(service, preset)
                } else {
                    format!("env/{}/{}.env", service, preset)
                };
                println!("\n  {} [{}]: .env differs from {}", label, preset, ref_label);
                let reload_label = format!("reload from {}", ref_label);
                let write_label = format!("write current .env → {}", settings.project.preset_local_rel(service, preset));
                let vault_label_opt = backend.map(|b| format!("push current .env → {}", b.label()));
                let mut opt_strs: Vec<(&str, &str)> = vec![
                    ("d", "show diff"),
                    ("r", reload_label.as_str()),
                    ("w", write_label.as_str()),
                ];
                if let Some(ref vl) = vault_label_opt {
                    opt_strs.push(("v", vl.as_str()));
                }
                opt_strs.push(("s", "skip"));
                let mut choice = menu(&opt_strs, "r");
                while choice == "d" {
                    show_diff(loc, r, &format!("current {}/{}", service, label), &ref_label);
                    choice = menu(&opt_strs, "r");
                }
                if choice == "r" {
                    crate::backup::backup_env_before_write(
                        label, local_path, "startup-sync-reload",
                        &format!("user reloaded from {} (no backend)", ref_label),
                    );
                    if let Err(e) = write_file(local_path, r) {
                        eprintln!("    error writing .env: {}", e);
                    } else {
                        println!("    Reloaded.");
                    }
                } else if choice == "w" {
                    let target = settings.project.preset_local_path(service, preset);
                    if let Some(parent) = target.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = std::fs::write(&target, loc) {
                        eprintln!("    error writing local cache: {}", e);
                    } else {
                        println!("    Saved to {}.", settings.project.preset_local_rel(service, preset));
                    }
                } else if choice == "v" {
                    if let Some(b) = backend {
                        push_with_bucket(b, service, preset, loc);
                        println!("    Pushed to {}.", b.label());
                    }
                } else {
                    println!("    Skipped.");
                }
            }
            (Some(_), None) => println!("  {} [{}]: ✓", label, preset),
        }
    }
}

// ── Entry point ────────────────────────────────────────────────────────────────

pub fn run(settings: &Settings) -> Result<()> {
    println!("=== {} morning check ===\n", settings.project.project_name);

    // Check backend availability
    let backend_box = crate::backend::active_backend(settings);

    let backend_ref: Option<&dyn SecretBackend> = backend_box.as_deref();

    if backend_ref.is_none() {
        let label = settings.backend_label();
        let msg = if label != "local-only" {
            format!("  {} unavailable", label)
        } else {
            "  No secret backend configured".to_string()
        };
        println!("{} — running in local-only mode.\n", msg);
    }

    let timestamp = {
        use std::time::{SystemTime, UNIX_EPOCH};
        // Simple timestamp without chrono dependency
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Format as YYYY-MM-DD_HH-MM-SS using a simple computation
        let s = secs as i64;
        let secs_per_day = 86400i64;
        let days = s / secs_per_day;
        let time_of_day = s % secs_per_day;
        let hh = time_of_day / 3600;
        let mm = (time_of_day % 3600) / 60;
        let ss = time_of_day % 60;
        // Convert days-since-epoch to a date (Gregorian)
        let date = days_to_date(days);
        format!(
            "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
            date.0, date.1, date.2, hh, mm, ss
        )
    };

    let mut backup = BackupSession::new(&timestamp);
    let mut any_work = false;

    // ── UI workspaces ──────────────────────────────────────────────────────────
    let ui_repo = settings.project.dev_ui.repo.as_str();
    let workspaces = all_tracked_workspaces();
    if !workspaces.is_empty() {
        println!("{} workspaces:", ui_repo);
    }

    for (ws_path, preset) in &workspaces {
        if !Path::new(ws_path).exists() {
            let short = Path::new(ws_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(ws_path);
            println!("  {}/{}: folder not found, skipping", ui_repo, short);
            any_work = true;
            continue;
        }
        let label = format!(
            "{}/{}",
            ui_repo,
            Path::new(ws_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(ws_path)
        );
        let local_path = settings.project.service_env_path(ui_repo, Some(ws_path));

        let ws_clone = ws_path.clone();
        let preset_clone = preset.clone();

        handle(
            &label,
            ui_repo,
            &local_path,
            preset,
            backend_ref,
            &mut backup,
            &|p: &str| {
                let _ = crate::state::set_workspace_preset(&ws_clone, p);
            },
            &|| {
                let mut state = State::load();
                state.remove_workspace(&ws_clone);
                let _ = state.save();
            },
            settings,
        );
        any_work = true;
        let _ = preset_clone; // suppress unused warning
    }

    // ── backend services ───────────────────────────────────────────────────────
    let services = all_tracked_services();
    if !services.is_empty() {
        println!("\nbackend services:");
    }

    for (service, preset) in &services {
        let local_path = settings.project.service_env_path(service, None);
        if !local_path.parent().map(|p| p.exists()).unwrap_or(false) {
            println!("  {}: repo folder not found, skipping", service);
            any_work = true;
            continue;
        }
        let svc_clone = service.clone();

        handle(
            service,
            service,
            &local_path,
            preset,
            backend_ref,
            &mut backup,
            &|p: &str| {
                let _ = crate::state::set_service_preset(&svc_clone, p);
            },
            &|| {
                let mut state = State::load();
                state.remove_service(&svc_clone);
                let _ = state.save();
            },
            settings,
        );
        any_work = true;
    }

    if !any_work {
        println!("  No tracked workspaces or services yet.");
        println!("  Run dev-ui.sh or dev-backend.sh once to start tracking.");
    }

    backup.flush();
    println!("\nDone.");
    Ok(())
}

/// Convert days since Unix epoch (1970-01-01) to (year, month, day).
fn days_to_date(days: i64) -> (i32, u32, u32) {
    // Use a simple algorithm — Gregorian calendar
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}
