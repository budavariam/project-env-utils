//! `penv setup-wizard` — guided 1Password setup.
//!
//! Walks through every prerequisite step interactively, pausing for the user
//! to complete each one before moving on. No docs needed.

use std::io::{self, BufRead, Write};

use anyhow::Result;

use crate::backend::onepassword::OpBackend;
use crate::config::{project_config_file, repo_root, settings_file, Settings};

// ── I/O helpers ────────────────────────────────────────────────────────────

fn print_header(n: usize, total: usize, title: &str) {
    println!();
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║  Step {n}/{total}  —  {title:<48}║");
    println!("╚══════════════════════════════════════════════════════════╝");
}

fn print_info(msg: &str) {
    for line in msg.lines() {
        println!("  {}", line);
    }
}

/// Block until the user presses Enter.
fn wait_enter(prompt: &str) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write!(out, "\n  ▶  {}  (press Enter) ", prompt).ok();
    out.flush().ok();
    let stdin = io::stdin();
    let _ = stdin.lock().lines().next();
}

/// Ask a question and return the trimmed answer (empty = default).
fn ask(prompt: &str, default: &str) -> String {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write!(out, "\n  {} [{}]: ", prompt, default).ok();
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

/// Run a command, stream its stdout/stderr live, return success flag.
fn run_live(args: &[&str]) -> bool {
    println!();
    println!("  $ {}", args.join(" "));
    println!();
    let status = std::process::Command::new(args[0])
        .args(&args[1..])
        .status();
    match status {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!("  error running {}: {}", args[0], e);
            false
        }
    }
}

/// Check if `op` is on PATH (doesn't require being signed in).
fn op_installed() -> bool {
    std::process::Command::new("op")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Read existing settings.local.json (or empty object).
fn read_local_settings() -> serde_json::Value {
    settings_file()
        .exists()
        .then(|| std::fs::read_to_string(settings_file()).ok())
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::json!({}))
}

/// Write settings.local.json, merging with existing content.
fn write_local_settings(patch: serde_json::Value) -> Result<()> {
    let mut current = read_local_settings();
    if let (Some(obj), Some(patch_obj)) = (current.as_object_mut(), patch.as_object()) {
        for (k, v) in patch_obj {
            obj.insert(k.clone(), v.clone());
        }
    }
    let content = serde_json::to_string_pretty(&current)?;
    std::fs::write(settings_file(), content)?;
    Ok(())
}

// ── Main wizard ─────────────────────────────────────────────────────────────

pub fn run(settings: &Settings) -> Result<()> {
    let total = 6usize;
    let default_vault = if settings.op_vault.is_empty() {
        project_config_file()
            .exists()
            .then(|| {
                std::fs::read_to_string(project_config_file()).ok().and_then(|s| {
                    serde_json::from_str::<serde_json::Value>(&s).ok()?.get("project")
                        ?.get("op_vault")?.as_str().map(str::to_string)
                })
            })
            .flatten()
            .unwrap_or_else(|| "Project Dev".to_string())
    } else {
        settings.op_vault.clone()
    };

    println!();
    println!("  ╔════════════════════════════════════════════════════════╗");
    println!("  ║       penv — 1Password setup wizard                ║");
    println!("  ║                                                        ║");
    println!("  ║  This wizard configures 1Password as your secret       ║");
    println!("  ║  backend, creates the vault, and pulls all presets.    ║");
    println!("  ║                                                        ║");
    println!("  ║  Total steps: {}                                        ║", total);
    println!("  ╚════════════════════════════════════════════════════════╝");

    wait_enter("Ready to start?");

    // ── Step 1: Check op CLI ────────────────────────────────────────────────
    print_header(1, total, "Check 1Password CLI");
    if op_installed() {
        print_info("✓  `op` CLI found.");
    } else {
        print_info("✗  `op` CLI not found on PATH.\n");
        print_info("Install it from:  https://1password.com/downloads/command-line/");
        print_info("");
        print_info("macOS (Homebrew):  brew install 1password-cli");
        print_info("                   brew install --cask 1password");
        print_info("");
        print_info("After installing, come back and press Enter.");
        wait_enter("I've installed op — continue?");
        if !op_installed() {
            anyhow::bail!("`op` still not found. Re-run the wizard after installing it.");
        }
        print_info("✓  `op` CLI found.");
    }

    // ── Step 2: Sign in ─────────────────────────────────────────────────────
    print_header(2, total, "Sign in to 1Password");
    print_info("You need to be signed in to the 1Password CLI.\n");
    print_info("Command:  op signin");
    print_info("");
    print_info("This will open a browser window or prompt for your");
    print_info("account password. Complete it, then come back here.");
    wait_enter("Run `op signin` in another terminal, then press Enter when done");

    // Verify sign-in by listing vaults (lightweight check).
    let signed_in = std::process::Command::new("op")
        .args(["vault", "list", "--format", "json"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !signed_in {
        print_info("⚠  Could not reach 1Password. Make sure you ran `op signin`.");
        print_info("   Continuing anyway — vault operations may fail.");
    } else {
        print_info("✓  1Password session active.");
    }

    // ── Step 3: Choose vault ────────────────────────────────────────────────
    print_header(3, total, "Choose vault name");
    print_info("Each project stores its secrets in a dedicated 1Password vault.");
    print_info(&format!("The project default is:  \"{}\"", default_vault));
    print_info("");
    print_info("Press Enter to accept the default, or type a different name.");
    let vault = ask("Vault name", &default_vault);
    print_info(&format!("→  Using vault: \"{}\"", vault));

    // ── Step 4: Write settings.local.json ──────────────────────────────────
    print_header(4, total, "Write settings.local.json");
    let settings_path = settings_file();
    print_info(&format!("Writing to:  {:?}", settings_path));
    print_info("");
    print_info(&format!(
        "  {{ \"secret_backend\": \"onepassword\", \"onepassword_vault\": \"{}\" }}",
        vault
    ));
    print_info("");
    print_info("This file is gitignored — it stays on your machine only.");
    wait_enter("Write settings.local.json?");

    write_local_settings(serde_json::json!({
        "secret_backend": "onepassword",
        "onepassword_vault": vault,
    }))?;
    print_info("✓  settings.local.json written.");

    // ── Step 5: Init vault ──────────────────────────────────────────────────
    print_header(5, total, "Init vault in 1Password");
    print_info(&format!("Command:  penv op init-vault"));
    print_info("");
    print_info("This verifies the vault exists and creates it if needed.");
    print_info("It also creates an index item listing all services.");
    wait_enter("Run init-vault?");

    let updated = Settings::load().unwrap_or_else(|_| settings.clone()); // reload with new settings.local.json
    let backend = OpBackend::new(&vault, &updated.project.op_item_prefix);
    let services: Vec<String> = updated.project.services.iter().map(|s| s.name.clone()).collect();
    backend.init_vault(&services)?;

    // ── Step 6: Sync presets ────────────────────────────────────────────────
    print_header(6, total, "Sync presets with 1Password");
    let available = crate::config::available_presets(
        updated.project.services.first().map(|s| s.name.as_str()).unwrap_or("my-service"),
    );
    let avail_str = if available.is_empty() {
        "test, uat, dev".to_string()
    } else {
        available.join(", ")
    };
    print_info(&format!("Available presets:  {}", avail_str));
    print_info("");
    print_info("What do you want to do?");
    print_info("");
    print_info("  pull  — download secrets from 1Password → local .env files");
    print_info("          (use this if the vault already has your secrets)");
    print_info("  push  — upload local .env files → 1Password");
    print_info("          (use this if you have .env files locally but vault is empty)");
    print_info("  skip  — do nothing now, sync manually later");
    print_info("");

    let exe = std::env::current_exe().unwrap_or_else(|_| repo_root().join("penv"));
    let exe_str = exe.to_str().unwrap_or("penv");

    loop {
        let action = ask("Action", "pull");
        match action.trim().to_lowercase().as_str() {
            "pull" => {
                let default_presets = available.join(", ");
                let raw = ask("Presets to pull (comma-separated)", &default_presets);
                let presets: Vec<String> = raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if presets.is_empty() { print_info("No presets entered."); continue; }
                print_info(&format!("\nWill run:"));
                for p in &presets { print_info(&format!("  penv op pull {}", p)); }
                wait_enter(&format!("Pull {} preset(s) from 1Password?", presets.len()));
                let mut all_ok = true;
                for p in &presets {
                    print_info(&format!("\n── pulling \"{}\" ──", p));
                    if !run_live(&[exe_str, "op", "pull", p]) { all_ok = false; }
                }
                if all_ok {
                    print_info(&format!("✓  All {} preset(s) pulled.", presets.len()));
                } else {
                    print_info("⚠  Some presets failed. Check the output above.");
                    print_info("   The vault may be empty — try \"push\" to seed it first.");
                }
                break;
            }
            "push" => {
                let default_presets = available.join(", ");
                let raw = ask("Presets to push (comma-separated)", &default_presets);
                let presets: Vec<String> = raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if presets.is_empty() { print_info("No presets entered."); continue; }
                print_info(&format!("\nWill run:"));
                for p in &presets { print_info(&format!("  penv op push {}", p)); }
                print_info("This uploads your local .env files to 1Password.");
                wait_enter(&format!("Push {} preset(s) to 1Password?", presets.len()));
                let mut all_ok = true;
                for p in &presets {
                    print_info(&format!("\n── pushing \"{}\" ──", p));
                    if !run_live(&[exe_str, "op", "push", p]) { all_ok = false; }
                }
                if all_ok {
                    print_info(&format!("✓  All {} preset(s) pushed.", presets.len()));
                } else {
                    print_info("⚠  Some presets failed. Check the output above.");
                }
                break;
            }
            "skip" | "" => {
                print_info("Skipped. Run manually when ready:");
                print_info("  penv op pull <preset>   or   penv op push <preset>");
                break;
            }
            other => {
                print_info(&format!("  Unknown action \"{}\". Type pull, push, or skip.", other));
            }
        }
    }

    // ── Done ────────────────────────────────────────────────────────────────
    println!();
    println!("  ╔══════════════════════════════════════════════════════════╗");
    println!("  ║  ✓  Setup complete!                                      ║");
    println!("  ║                                                          ║");
    println!("  ║  You're now using 1Password as your secret backend.      ║");
    println!("  ║                                                          ║");
    println!("  ║  Useful next commands:                                   ║");
    println!("  ║    penv op pull <preset>   pull another preset       ║");
    println!("  ║    penv op push <preset>   push local → 1Password    ║");
    println!("  ║    penv op list            list presets in vault      ║");
    println!("  ║    penv morning-check      start-of-day sync check   ║");
    println!("  ╚══════════════════════════════════════════════════════════╝");
    println!();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::ENV_LOCK;

    #[test]
    fn write_local_settings_creates_file_with_backend_and_vault() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("PENV_REPO_ROOT", dir.path().to_str().unwrap());

        write_local_settings(serde_json::json!({
            "secret_backend": "onepassword",
            "onepassword_vault": "My Vault",
        })).unwrap();

        let content = std::fs::read_to_string(dir.path().join("settings.local.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["secret_backend"], "onepassword");
        assert_eq!(v["onepassword_vault"], "My Vault");

        std::env::remove_var("PENV_REPO_ROOT");
    }

    #[test]
    fn write_local_settings_merges_without_overwriting_other_keys() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("PENV_REPO_ROOT", dir.path().to_str().unwrap());

        // Pre-populate with an existing key
        std::fs::write(
            dir.path().join("settings.local.json"),
            r#"{"other_key": true}"#,
        ).unwrap();

        write_local_settings(serde_json::json!({
            "secret_backend": "onepassword",
        })).unwrap();

        let content = std::fs::read_to_string(dir.path().join("settings.local.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["secret_backend"], "onepassword");
        // Existing keys must be preserved
        assert_eq!(v["other_key"], true);

        std::env::remove_var("PENV_REPO_ROOT");
    }
}
