//! `penv validate-cache [--fix]`
//!
//! Checks that every `local/<project>/<service>/<preset>.env` file matches
//! what is stored in the active backend (default: SQLite).
//! With `--fix`, backs up the current backend content and pushes local cache →
//! backend for any pair that differs.
//! Exits non-zero if anything is out of sync (and --fix was not used).
use anyhow::Result;

use crate::backend::SecretBackend;
use crate::cmd::sync::{resolve_use_color, unified_diff};
use crate::config::{Settings, available_presets_for_project, backups_dir};

fn timestamp() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let s = dur.as_secs();
    let sec = s % 60;
    let min = (s / 60) % 60;
    let hour = (s / 3600) % 24;
    let mut days = s / 86400;
    let mut year = 1970u64;
    loop {
        let y_days = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
            366
        } else {
            365
        };
        if days < y_days {
            break;
        }
        days -= y_days;
        year += 1;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let months = [
        31u64,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u64;
    for m in &months {
        if days < *m {
            break;
        }
        days -= m;
        month += 1;
    }
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        year,
        month,
        days + 1,
        hour,
        min,
        sec
    )
}

pub fn run(fix: bool, backend: &dyn SecretBackend, settings: &Settings) -> Result<()> {
    let project = &settings.project.project_name;
    let use_color = resolve_use_color(settings, false);
    let mut any_bad = false;

    // One backup directory for the entire --fix run.
    let backup_dir = if fix {
        let ts = timestamp();
        let dir = backups_dir().join(format!("{}-validate-cache-fix", ts));
        std::fs::create_dir_all(&dir).ok();
        Some(dir)
    } else {
        None
    };

    let backend_pairs: std::collections::HashSet<(String, String)> =
        backend.list().into_iter().collect();

    for svc in &settings.project.services {
        let presets = available_presets_for_project(&svc.name, project);

        for preset in &presets {
            let local_path = settings.project.preset_local_path(&svc.name, preset);
            let local = match std::fs::read_to_string(&local_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            match backend.fetch(&svc.name, preset) {
                None => {
                    if fix {
                        if backend.push(&svc.name, preset, &local) {
                            println!(
                                "  ↑ {}/{}: pushed to {} (was absent)",
                                svc.name,
                                preset,
                                backend.label()
                            );
                        } else {
                            eprintln!("  ✗ {}/{}: push failed", svc.name, preset);
                            any_bad = true;
                        }
                    } else {
                        println!(
                            "  ✗ {}/{}: local cache exists but not in {}",
                            svc.name,
                            preset,
                            backend.label()
                        );
                        any_bad = true;
                    }
                }
                Some(remote) if local == remote => {
                    println!("  ✓ {}/{}: in sync", svc.name, preset);
                }
                Some(remote) => {
                    if fix {
                        if let Some(ref dir) = backup_dir {
                            let fname = format!("{}.{}.before.env", svc.name, preset);
                            let _ = std::fs::write(dir.join(&fname), &remote);
                        }
                        if backend.push(&svc.name, preset, &local) {
                            println!("  ↑ {}/{}: pushed to {}", svc.name, preset, backend.label());
                        } else {
                            eprintln!("  ✗ {}/{}: push failed", svc.name, preset);
                            any_bad = true;
                        }
                    } else {
                        println!("\n  ✗ {}/{}: differs", svc.name, preset);
                        let from = format!("{} {}/{}", backend.label(), svc.name, preset);
                        let to = settings.project.preset_local_rel(&svc.name, preset);
                        print!(
                            "{}",
                            unified_diff(&remote, &local, &from, &to, usize::MAX, use_color)
                        );
                        any_bad = true;
                    }
                }
            }
        }

        // Flag backend entries with no local cache file.
        for (b_svc, b_preset) in &backend_pairs {
            if b_svc != &svc.name {
                continue;
            }
            let local_path = settings.project.preset_local_path(&svc.name, b_preset);
            if !local_path.exists() {
                println!(
                    "  ✗ {}/{}: in {} but no local cache file",
                    svc.name,
                    b_preset,
                    backend.label()
                );
                any_bad = true;
            }
        }
    }

    if any_bad {
        anyhow::bail!(
            "cache and {} are out of sync — re-run with --fix to push local → {}",
            backend.label(),
            backend.label()
        );
    }
    if fix {
        if let Some(ref dir) = backup_dir {
            println!("\nBackups written to {}", dir.display());
        }
        println!("Done.");
    } else {
        println!("\nAll local cache files match {}.", backend.label());
    }
    Ok(())
}
