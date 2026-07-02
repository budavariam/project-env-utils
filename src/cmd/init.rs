//! `penv init` — guided first-time setup: creates settings.json for a new project.
//!
//! Asks for the project name, vault, service repos, and optionally launches
//! the secret backend wizard. No existing config needed.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use anyhow::Result;

use crate::config::repo_root;

// ── I/O helpers ────────────────────────────────────────────────────────────

fn ask(prompt: &str, default: &str) -> String {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if default.is_empty() {
        write!(out, "  {}: ", prompt).ok();
    } else {
        write!(out, "  {}: [{}]  ", prompt, default).ok();
    }
    out.flush().ok();
    let stdin = io::stdin();
    match stdin.lock().lines().next() {
        Some(Ok(l)) => {
            let v = l.trim().to_string();
            if v.is_empty() { default.to_string() } else { v }
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
    let answer = stdin.lock().lines().next().and_then(|l| l.ok())
        .map(|l| l.trim().to_lowercase()).unwrap_or_default();
    if answer.is_empty() { default_yes } else { answer.starts_with('y') }
}

fn print_step(n: usize, title: &str) {
    println!();
    println!("  ── {} ─── {}", n, title);
}

// ── Main ───────────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let settings_path = repo_root().join("settings.json");

    println!();
    println!("  ╔══════════════════════════════════════════════════════╗");
    println!("  ║   penv init — create settings.json               ║");
    println!("  ║                                                      ║");
    println!("  ║   This wizard creates a settings.json for your       ║");
    println!("  ║   project. You can edit it later at any time.        ║");
    println!("  ╚══════════════════════════════════════════════════════╝");
    println!();

    if settings_path.exists() {
        println!("  ⚠  settings.json already exists at {:?}", settings_path);
        if !ask_yn("Overwrite it?", false) {
            println!("  Aborted.");
            return Ok(());
        }
    }

    // ── 1. Project name ─────────────────────────────────────────────────────
    print_step(1, "Project name");
    println!("  Used as the base label in logs and 1Password.");
    let project_name = ask("Project name", "MyProject");

    // ── 2. 1Password vault ─────────────────────────────────────────────────
    print_step(2, "1Password vault");
    println!("  Name of the vault in 1Password where secrets are stored.");
    println!("  It will be created if it doesn't exist (via setup-wizard).");
    let op_vault = ask("Vault name", &format!("{} Dev", project_name));

    // ── 3. Item prefix ──────────────────────────────────────────────────────
    print_step(3, "Item prefix");
    println!("  Prefix for item titles in 1Password, e.g. 'myproj' → 'myproj/my-api'.");
    println!("  Helps identify this project's items when you share a vault.");
    let default_prefix = project_name.to_lowercase().replace(' ', "-");
    let op_item_prefix = ask("Prefix (leave blank for none)", &default_prefix);
    let bucket = String::new(); // not used — local-only fallback

    // ── 5. Services ─────────────────────────────────────────────────────────
    print_step(5, "Service repos");
    println!("  Each service needs a name (= repo folder name) and optionally");
    println!("  a description and key env vars to display in 'show-info'.");
    println!();
    println!("  Enter each service, one per line.  Type an empty name to stop.");
    println!("  Format:  name  [description]");
    println!("  Example: my-api   Gateway API");

    let mut services: Vec<serde_json::Value> = Vec::new();
    let mut idx = 1;
    loop {
        println!();
        let raw = ask(&format!("Service {} name (empty to stop)", idx), "");
        if raw.is_empty() { break; }

        // Split on first whitespace — rest is description
        let (name, rest) = raw.split_once(char::is_whitespace)
            .map(|(n, r)| (n.trim().to_string(), r.trim().to_string()))
            .unwrap_or((raw.trim().to_string(), String::new()));

        let description = if rest.is_empty() {
            ask(&format!("  Description for '{}' (optional)", name), "")
        } else {
            rest
        };

        let env_vars_raw = ask(
            &format!("  Key env vars for '{}' (comma-separated, optional)", name),
            "",
        );
        let env_vars: Vec<String> = env_vars_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut svc = serde_json::json!({ "name": name });
        if !description.is_empty() {
            svc["description"] = serde_json::Value::String(description);
        }
        if !env_vars.is_empty() {
            svc["env_vars"] = serde_json::Value::Array(
                env_vars.into_iter().map(serde_json::Value::String).collect(),
            );
        }
        services.push(svc);
        idx += 1;
    }

    if services.is_empty() {
        println!("  No services added — you can edit settings.json later.");
    }

    // ── 6. UI repo (optional) ───────────────────────────────────────────────
    print_step(6, "UI repo (optional)");
    println!("  The frontend repo for 'penv dev-ui'.");
    let ui_repo_raw = ask("UI repo name (empty to skip)", "");
    let dev_ui = if !ui_repo_raw.is_empty() {
        let pane_dev = ask("  Dev command", "npm install && npm run dev");
        let pane_sb  = ask("  Storybook command", "npm run storybook");
        Some(serde_json::json!({
            "repo": ui_repo_raw,
            "pane_dev_cmd": pane_dev,
            "pane_sb_cmd": pane_sb,
        }))
    } else {
        None
    };

    // ── 7. Backend session (optional) ───────────────────────────────────────
    print_step(7, "Backend session (optional)");
    println!("  Repos opened in 'penv dev-backend'.");
    let session_name = ask("tmux session name (empty to skip)", "");
    let dev_backend = if !session_name.is_empty() {
        println!("  Enter backend repos (empty name to stop):");
        let mut panes: Vec<serde_json::Value> = Vec::new();
        let mut pidx = 1;
        loop {
            let repo = ask(&format!("  Repo {} (empty to stop)", pidx), "");
            if repo.is_empty() { break; }
            let cmd = ask(&format!("  Start command for '{}'", repo), "npm install && npm run dev");
            panes.push(serde_json::json!({ "repo": repo, "cmd": cmd }));
            pidx += 1;
        }
        Some(serde_json::json!({
            "session_name": session_name,
            "window_backend": panes,
            "window_service": [],
        }))
    } else {
        None
    };

    // ── Build settings.json ──────────────────────────────────────────────────
    let mut project_node = serde_json::json!({
        "name": project_name,
        "op_vault": op_vault,
        "op_item_prefix": op_item_prefix,
    });
    let mut config = serde_json::json!({
        "project": project_node,
        "services": services,
    });

    if let Some(ui) = dev_ui {
        config["dev_ui"] = ui;
    }
    if let Some(be) = dev_backend {
        config["dev_backend"] = be;
    }

    // ── Preview + confirm ────────────────────────────────────────────────────
    println!();
    println!("  ── Preview ──────────────────────────────────────────");
    println!("{}", serde_json::to_string_pretty(&config)?
        .lines()
        .map(|l| format!("  {}", l))
        .collect::<Vec<_>>()
        .join("\n"));
    println!();
    if !ask_yn(&format!("Write to {:?}?", settings_path), true) {
        println!("  Aborted.");
        return Ok(());
    }

    std::fs::write(&settings_path, serde_json::to_string_pretty(&config)?)?;
    println!("  ✓  settings.json written.");

    // ── Offer setup-wizard ───────────────────────────────────────────────────
    println!();
    if ask_yn("Run the 1Password setup wizard now?", true) {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("penv"));
        let _ = std::process::Command::new(&exe)
            .arg("setup-wizard")
            .status();
    } else {
        println!();
        println!("  Done! Next steps:");
        println!("    penv setup-wizard   configure 1Password backend");
        println!("    penv op push test   push your first preset");
        println!("    penv export         export .env files per service");
    }
    println!();

    Ok(())
}
