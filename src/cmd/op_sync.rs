/// `penv op <subcommand>` — 1Password sync commands.
use std::io::{self, BufRead, Write};

use anyhow::Result;

use crate::backend::onepassword::OpBackend;
use crate::backend::SecretBackend;
use crate::cmd::sync;
use crate::config::Settings;

fn require_op(backend: &OpBackend) {
    if !backend.available() {
        eprintln!(
            "ERROR: 1Password CLI is not available or not signed in.\n\
             Make sure `op` is installed and run `op signin` first.\n\
             Also set {{\"secret_backend\": \"onepassword\"}} in settings.local.json."
        );
        std::process::exit(2);
    }
}

fn make_backend(settings: &Settings) -> OpBackend {
    let descriptions: std::collections::HashMap<String, String> = settings
        .project
        .services
        .iter()
        .filter(|s| !s.description.is_empty())
        .map(|s| (s.name.clone(), s.description.clone()))
        .collect();
    OpBackend::new(&settings.op_vault, &settings.project.op_item_prefix)
        .with_descriptions(descriptions)
}

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

pub fn run_push(
    preset: &str,
    service: Option<&str>,
    settings: &Settings,
    dry_run: bool,
) -> Result<()> {
    let b = make_backend(settings);
    require_op(&b);
    sync::cmd_push(preset, service, &b, settings, dry_run);
    Ok(())
}

pub fn run_pull(
    preset: &str,
    service: Option<&str>,
    settings: &Settings,
    dry_run: bool,
) -> Result<()> {
    let b = make_backend(settings);
    require_op(&b);
    sync::cmd_pull(preset, service, &b, settings, dry_run);
    Ok(())
}

pub fn run_diff(preset: &str, service: Option<&str>, settings: &Settings) -> Result<()> {
    let b = make_backend(settings);
    require_op(&b);
    sync::cmd_diff(preset, service, &b, settings);
    Ok(())
}

pub fn run_list(service: Option<&str>, settings: &Settings) -> Result<()> {
    let b = make_backend(settings);
    require_op(&b);
    sync::cmd_list(service, &b, settings);
    Ok(())
}

pub fn run_init_vault(settings: &Settings) -> Result<()> {
    let b = make_backend(settings);
    require_op(&b);
    let services: Vec<String> = settings
        .project
        .services
        .iter()
        .map(|s| s.name.clone())
        .collect();
    b.init_vault(&services)
}

/// Interactive: copy an existing preset to a new name across chosen services.
pub fn run_new_preset(settings: &Settings) -> Result<()> {
    let b = make_backend(settings);
    require_op(&b);

    // ── Discover existing presets ───────────────────────────────────────────
    let all_pairs = b.list();
    let mut existing_presets: Vec<String> = all_pairs
        .iter()
        .map(|(_, p)| p.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    existing_presets.sort();

    let all_services: Vec<String> = settings
        .project
        .services
        .iter()
        .map(|s| s.name.clone())
        .collect();

    println!();
    println!("  ╔══════════════════════════════════════════════════════╗");
    println!("  ║   penv op new-preset — copy a preset             ║");
    println!("  ╚══════════════════════════════════════════════════════╝");
    println!();

    if existing_presets.is_empty() {
        println!("  No presets found in 1Password. Push some first:");
        println!("    penv op push test");
        return Ok(());
    }

    // ── Choose source preset ────────────────────────────────────────────────
    println!("  Existing presets:  {}", existing_presets.join("  |  "));
    println!();
    let src = ask(
        "Copy FROM preset",
        existing_presets
            .first()
            .map(|s| s.as_str())
            .unwrap_or("test"),
    );

    // ── Choose new preset name ──────────────────────────────────────────────
    println!();
    let dest = ask("New preset name", "");
    if dest.is_empty() {
        anyhow::bail!("New preset name cannot be empty.");
    }
    if existing_presets.contains(&dest) {
        anyhow::bail!("Preset '{}' already exists. Choose a different name.", dest);
    }

    // ── Choose services ─────────────────────────────────────────────────────
    println!();
    println!("  Services:  {}", all_services.join("  |  "));
    let svc_input = ask("Services to copy (comma-separated, or 'all')", "all");
    let services: Vec<String> = if svc_input.trim().eq_ignore_ascii_case("all") {
        all_services.clone()
    } else {
        svc_input
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    // ── Summary ─────────────────────────────────────────────────────────────
    println!();
    println!(
        "  Will copy:  {} → {}  for {} service(s):",
        src,
        dest,
        services.len()
    );
    for svc in &services {
        println!("    {}", svc);
    }
    println!();
    {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        write!(out, "  Proceed? [Y/n]  ").ok();
        out.flush().ok();
    }
    let stdin = io::stdin();
    let answer = stdin
        .lock()
        .lines()
        .next()
        .and_then(|l| l.ok())
        .map(|l| l.trim().to_lowercase())
        .unwrap_or_default();
    if answer == "n" || answer == "no" {
        println!("  Aborted.");
        return Ok(());
    }

    // ── Copy ────────────────────────────────────────────────────────────────
    println!();
    let mut ok_count = 0usize;
    for svc in &services {
        // Fetch the source content from 1Password
        let content = match b.fetch(svc, &src) {
            Some(c) => c,
            None => {
                println!("  ✗  {}  — '{}' not found in 1Password, skipping", svc, src);
                continue;
            }
        };

        // Push as the new preset name
        if b.push(svc, &dest, &content) {
            // Also write to local .env so it's immediately usable
            let dest_path = settings.project.service_env_path(svc, None);
            crate::backup::backup_env_before_write(
                &format!("{}/{}", svc, dest),
                &dest_path,
                "op-new-preset",
                &format!("new-preset: {} copied from {} → {}", svc, src, dest),
            );
            crate::backup::log_backend_push(
                &format!("{}/{}", svc, dest),
                "op-new-preset",
                &format!(
                    "pushed new preset {} to {} (copied from {})",
                    dest,
                    b.label(),
                    src
                ),
            );
            let _ = crate::env_file::write_file(&dest_path, &content);
            println!("  ✓  {}  [{}] copied from [{}]", svc, dest, src);
            let _ = crate::state::set_service_preset(svc, &dest);
            ok_count += 1;
        } else {
            println!("  ✗  {}  — push failed", svc);
        }
    }

    println!();
    println!("  {}/{} services copied.", ok_count, services.len());
    if ok_count > 0 {
        println!();
        println!(
            "  New preset '{}' is now active. Edit .env files as needed,",
            dest
        );
        println!("  then push changes back with:");
        println!("    penv op push {}", dest);
    }
    println!();

    Ok(())
}
