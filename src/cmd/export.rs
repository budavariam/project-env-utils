//! `penv export` — interactively export a preset per service to a folder.
//!
//! Prompts for which preset to use for each service independently,
//! then for a destination folder, then optionally zips the result.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::cmd::pick_preset::load_service_for_pick;
use crate::config::{available_presets, repo_parent, Settings};
use crate::env_file::write_file;
use crate::state::{get_service_preset, set_service_preset};

// ── I/O helpers ────────────────────────────────────────────────────────────

fn ask(prompt: &str, default: &str) -> String {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write!(out, "  {}: [{}]  ", prompt, default).ok();
    out.flush().ok();
    let stdin = io::stdin();
    match stdin.lock().lines().next() {
        Some(Ok(l)) => {
            let v = l.trim().to_string();
            if v.is_empty() {
                default.to_string()
            } else {
                v
            }
        }
        _ => default.to_string(),
    }
}

fn ask_yn(prompt: &str, default_yes: bool) -> bool {
    let hint = if default_yes { "Y/n" } else { "y/N" };
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write!(out, "  {} [{}]  ", prompt, hint).ok();
    out.flush().ok();
    let stdin = io::stdin();
    let answer = stdin
        .lock()
        .lines()
        .next()
        .and_then(|l| l.ok())
        .map(|l| l.trim().to_lowercase())
        .unwrap_or_default();
    if answer.is_empty() {
        default_yes
    } else {
        answer.starts_with('y')
    }
}

fn print_divider() {
    println!("  {}", "─".repeat(54));
}

/// Expand a leading `~` to the home directory.
fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

/// Zip all .env files in `dir` into `zip_path` using the system `zip` command.
fn zip_dir(dir: &Path, zip_path: &Path) -> Result<()> {
    // Collect *.env files
    let files: Vec<String> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("env"))
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect();

    if files.is_empty() {
        anyhow::bail!("No .env files found in {:?} to zip", dir);
    }

    // Remove existing zip so we get a clean archive
    let _ = std::fs::remove_file(zip_path);

    let status = std::process::Command::new("zip")
        .arg(zip_path)
        .args(&files)
        .current_dir(dir)
        .status()?;

    if !status.success() {
        anyhow::bail!("zip exited with status {}", status);
    }
    Ok(())
}

// ── Main ───────────────────────────────────────────────────────────────────

pub fn run(settings: &Settings) -> Result<()> {
    let backend_box = crate::backend::active_backend(settings);
    let backend = backend_box.as_deref();

    let services: Vec<String> = settings
        .project
        .services
        .iter()
        .map(|s| s.name.clone())
        .collect();

    if services.is_empty() {
        anyhow::bail!("No services configured in settings.json");
    }

    println!();
    println!("  ╔══════════════════════════════════════════════════════╗");
    println!("  ║   penv export — choose a preset per service      ║");
    println!("  ║                                                      ║");
    println!("  ║   For each service, press Enter to accept the        ║");
    println!("  ║   default (shown in brackets) or type a new name.   ║");
    println!("  ╚══════════════════════════════════════════════════════╝");
    println!();
    println!("  Backend:  {}", settings.backend_label());
    println!("  Services: {}", services.join(", "));
    println!();

    let mut chosen: Vec<(String, String)> = Vec::new();

    // ── Per-service preset selection ────────────────────────────────────────
    for service in &services {
        print_divider();
        let available = available_presets(service);
        let last = get_service_preset(service);

        let default = last
            .as_deref()
            .or_else(|| available.first().map(|s| s.as_str()))
            .unwrap_or("test")
            .to_string();

        println!();
        println!("  Service:   {}", service);
        if !available.is_empty() {
            println!("  Presets:   {}", available.join("  |  "));
        }
        if let Some(ref l) = last {
            println!("  Last used: {}", l);
        }
        println!();

        let preset = ask(&format!("Preset for {}", service), &default);
        chosen.push((service.clone(), preset));
    }

    // ── Destination folder ─────────────────────────────────────────────────
    print_divider();
    println!();
    let default_dest = repo_parent().join("local").to_string_lossy().into_owned();
    println!("  Where should the .env files be written?");
    println!("  (Each file will be named <service>.<preset>.env)");
    println!();
    let dest_str = ask("Export folder", &default_dest);
    let dest_dir = expand_tilde(&dest_str);

    // ── Zip? ───────────────────────────────────────────────────────────────
    println!();
    let do_zip = ask_yn("Zip the exported files?", false);
    let zip_path = if do_zip {
        let default_zip = dest_dir
            .join("env-export.zip")
            .to_string_lossy()
            .into_owned();
        println!();
        let zp = ask("Zip file path", &default_zip);
        Some(expand_tilde(&zp))
    } else {
        None
    };

    // ── Summary + confirm ──────────────────────────────────────────────────
    print_divider();
    println!();
    println!("  About to export:");
    println!();
    for (svc, preset) in &chosen {
        println!(
            "    {}  [{}]  →  {}/{}.{}.env",
            svc,
            preset,
            dest_dir.display(),
            svc,
            preset
        );
    }
    if let Some(ref zp) = zip_path {
        println!();
        println!("  Then zip to:  {}", zp.display());
    }
    println!();
    if !ask_yn("Proceed?", true) {
        println!("  Aborted.");
        return Ok(());
    }

    // ── Create destination folder ──────────────────────────────────────────
    std::fs::create_dir_all(&dest_dir)?;

    // ── Pull / write to dest_dir ────────────────────────────────────────────
    println!();
    let mut ok_count = 0usize;
    let mut written_paths: Vec<PathBuf> = Vec::new();

    for (service, preset) in &chosen {
        // Write to a temp path in dest_dir using <service>.<preset>.env naming.
        let dest_file = dest_dir.join(format!("{}.{}.env", service, preset));

        // Use load_service_for_pick via a temp path in dest_dir, then rename.
        // We write to a temp path inside dest_dir so the parent exists.
        let tmp_path = dest_dir.join(format!("_{}.env.tmp", service));
        let (ok, status) = load_service_for_pick(service, preset, &tmp_path, backend, settings);

        if ok {
            // Rename tmp → final name
            if std::fs::rename(&tmp_path, &dest_file).is_err() {
                // rename may fail across devices; fall back to copy+delete
                if let Ok(content) = std::fs::read_to_string(&tmp_path) {
                    let _ = write_file(&dest_file, &content);
                    let _ = std::fs::remove_file(&tmp_path);
                }
            }
            println!(
                "  ✓  {}  [{}]  →  {}.{}.env  ({})",
                service, preset, service, preset, status
            );
            let _ = set_service_preset(service, preset);
            written_paths.push(dest_file);
            ok_count += 1;
        } else {
            let _ = std::fs::remove_file(&tmp_path);
            println!("  ✗  {}  [{}]  — {}", service, preset, status);
        }
    }

    // ── Zip ────────────────────────────────────────────────────────────────
    if let Some(ref zp) = zip_path {
        if ok_count > 0 {
            println!();
            print!("  Zipping {} file(s)...", ok_count);
            io::stdout().flush().ok();
            match zip_dir(&dest_dir, zp) {
                Ok(()) => {
                    let size = std::fs::metadata(zp)
                        .map(|m| format!("{} KB", m.len() / 1024))
                        .unwrap_or_else(|_| "?".to_string());
                    println!("  done  ({size})");
                    println!("  ✓  {}", zp.display());
                }
                Err(e) => println!("  ✗  zip failed: {}", e),
            }
        }
    }

    println!();
    print_divider();
    println!();
    println!("  {}/{} services exported.", ok_count, chosen.len());

    if ok_count < chosen.len() {
        println!();
        println!("  Some services failed. Check output above.");
        println!("  If the backend is empty, push first:");
        println!("    penv op push <preset>");
    } else {
        println!();
        println!("  Files in:  {}", dest_dir.display());
        if let Some(ref zp) = zip_path {
            println!("  Zip:       {}", zp.display());
        }
    }
    println!();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_replaces_leading_tilde() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let result = expand_tilde("~/foo/bar");
        assert_eq!(result, std::path::PathBuf::from(&home).join("foo/bar"));
    }

    #[test]
    fn expand_tilde_leaves_absolute_path_unchanged() {
        let result = expand_tilde("/absolute/path");
        assert_eq!(result, std::path::PathBuf::from("/absolute/path"));
    }

    #[test]
    fn expand_tilde_leaves_relative_path_unchanged() {
        let result = expand_tilde("relative/path");
        assert_eq!(result, std::path::PathBuf::from("relative/path"));
    }

    #[test]
    fn expand_tilde_bare_tilde_slash_expands_to_home() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let result = expand_tilde("~/");
        assert!(result.starts_with(&home));
    }
}
