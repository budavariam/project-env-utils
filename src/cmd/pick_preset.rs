//! `penv pick-preset (--workspace <path> | --backend) [--preset <name>]`
//!
//! Interactive preset selector.
//! stdout: exactly one line `PRESET:<chosen>` — parsed by shell scripts.
//! stderr: all human-readable prompts and status.
use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::Result;

use crate::backend::SecretBackend;
use crate::config::{Settings, available_presets, fallback_profile_path};
use crate::env_file::write_file;
use crate::state::{
    get_service_preset, get_workspace_preset, set_service_preset, set_workspace_preset,
};

fn err(msg: &str) {
    eprintln!("{}", msg);
}

fn prompt_preset(label: &str, last: Option<&str>, available: &[String]) -> String {
    let default = last
        .map(|s| s.to_string())
        .or_else(|| available.first().cloned())
        .unwrap_or_else(|| "test".to_string());

    let hint = if last.is_some() {
        format!("[{}] (last used)", default)
    } else {
        format!("[{}] (default)", default)
    };

    let mut lines = vec![format!("  Preset for {}?", label)];
    if !available.is_empty() {
        lines.push(format!("  Available: {}", available.join(", ")));
    }
    lines.push(format!("  {}  Enter preset: ", hint));

    let stderr = io::stderr();
    let mut se = stderr.lock();
    write!(se, "\n{}", lines[..lines.len() - 1].join("\n")).ok();
    write!(se, "\n{}", lines[lines.len() - 1]).ok();
    se.flush().ok();

    let stdin = io::stdin();
    match stdin.lock().lines().next() {
        Some(Ok(l)) => {
            let val = l.trim().to_string();
            if val.is_empty() { default } else { val }
        }
        _ => {
            eprintln!();
            std::process::exit(0);
        }
    }
}

pub fn load_service_for_pick(
    service: &str,
    preset: &str,
    dest: &Path,
    backend: Option<&dyn SecretBackend>,
    settings: &Settings,
) -> (bool, String) {
    if !dest.parent().map(|p| p.exists()).unwrap_or(false) {
        return (
            false,
            format!("skip — {:?} not found", dest.parent().unwrap_or(dest)),
        );
    }

    let label = format!("{}/{}", service, preset);

    if let Some(b) = backend {
        if let Some(content) = b.fetch(service, preset) {
            crate::backup::backup_env_before_write(
                &label,
                dest,
                "load-service-for-pick",
                &format!("loading from {}", b.label()),
            );
            if write_file(dest, &content).is_ok() {
                return (
                    true,
                    format!(
                        "{}:{}",
                        b.label().to_lowercase(),
                        b.key_display(service, preset)
                    ),
                );
            }
        }
        err(&format!(
            "  warn: '{}' not in {}, trying local",
            b.key_display(service, preset),
            settings.backend_label()
        ));
    }

    let preset_local = settings.project.preset_local_path(service, preset);
    if preset_local.exists()
        && let Ok(content) = std::fs::read_to_string(&preset_local)
    {
        crate::backup::backup_env_before_write(
            &label,
            dest,
            "load-service-for-pick",
            &format!(
                "loading from local cache {}",
                settings.project.preset_local_rel(service, preset)
            ),
        );
        if write_file(dest, &content).is_ok() {
            return (true, format!("{} local", preset));
        }
    }

    let fallback = settings.project.local_fallback_path(service);
    if fallback.exists()
        && let Ok(content) = std::fs::read_to_string(&fallback)
    {
        crate::backup::backup_env_before_write(
            &label,
            dest,
            "load-service-for-pick",
            &format!(
                "loading from generic local fallback {}",
                settings.project.local_fallback_rel(service)
            ),
        );
        if write_file(dest, &content).is_ok() {
            return (true, "local fallback (DB may not match preset)".to_string());
        }
    }

    let profile = fallback_profile_path(service, preset);
    if profile.exists()
        && let Ok(content) = std::fs::read_to_string(&profile)
    {
        crate::backup::backup_env_before_write(
            &label,
            dest,
            "load-service-for-pick",
            &format!(
                "loading from git-tracked profile env/{}/{}.env",
                service, preset
            ),
        );
        if write_file(dest, &content).is_ok() {
            return (true, "profile (no secrets)".to_string());
        }
    }

    (false, format!("no source found for {}/{}", service, preset))
}

/// Resolve (and load) the preset for a workspace.
/// `service` is the repo/service name used for state tracking and env loading.
pub fn resolve_workspace_preset(
    service: &str,
    workspace: &str,
    preset_override: Option<&str>,
    backend: Option<&dyn SecretBackend>,
    settings: &Settings,
) -> Result<String> {
    let last = get_workspace_preset(workspace);
    let avail = available_presets(service);
    let ws_name = Path::new(workspace)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(workspace);
    let label = format!("{}  ({})", service, ws_name);

    let preset = if let Some(p) = preset_override {
        p.to_string()
    } else {
        prompt_preset(&label, last.as_deref(), &avail)
    };

    let dest = settings.project.service_env_path(service, Some(workspace));

    // If the service .env exists and is newer than the penv cache, the user
    // likely edited it manually. Skip the silent pull so startup_sync_check
    // can show the diff and let them choose direction.
    let cache = settings.project.preset_local_path(service, &preset);
    let dest_mtime = std::fs::metadata(&dest).and_then(|m| m.modified()).ok();
    let cache_mtime = std::fs::metadata(&cache).and_then(|m| m.modified()).ok();
    let user_edited = matches!((dest_mtime, cache_mtime), (Some(d), Some(c)) if d > c)
        || matches!((dest_mtime, cache_mtime), (Some(_), None) if dest.exists());
    if user_edited {
        err(&format!(
            "  {} [{}]: .env is newer than cache — skipping auto-load, will sync-check",
            service, preset
        ));
        let _ = set_workspace_preset(workspace, &preset);
        return Ok(preset);
    }

    let (ok, status) = load_service_for_pick(service, &preset, &dest, backend, settings);
    err(&format!(
        "  loaded {} [{}] → {:?}  ({})",
        service, preset, dest, status
    ));
    if ok {
        let _ = set_workspace_preset(workspace, &preset);
    } else {
        anyhow::bail!(
            "no env source found for {}/{} — cannot continue",
            service,
            preset
        );
    }
    Ok(preset)
}

/// Resolve (and load) the backend preset for all window_backend services.
pub fn resolve_backend_preset(
    preset_override: Option<&str>,
    backend: Option<&dyn SecretBackend>,
    settings: &Settings,
) -> Result<String> {
    let window_backend = &settings.project.dev_backend.window_backend;
    let primary = window_backend
        .first()
        .map(|p| p.repo.as_str())
        .unwrap_or("backend");

    let last = get_service_preset(primary);
    let avail = available_presets(primary);

    let label = if window_backend.len() == 1 {
        primary.to_string()
    } else {
        let names: Vec<&str> = window_backend.iter().map(|p| p.repo.as_str()).collect();
        names.join(" + ")
    };

    let preset = if let Some(p) = preset_override {
        p.to_string()
    } else {
        prompt_preset(&label, last.as_deref(), &avail)
    };

    for pane in window_backend {
        let dest = settings.project.service_env_path(&pane.repo, None);
        let (ok, status) = load_service_for_pick(&pane.repo, &preset, &dest, backend, settings);
        err(&format!(
            "  loaded {} [{}] → {:?}  ({})",
            pane.repo, preset, dest, status
        ));
        if ok {
            let _ = set_service_preset(&pane.repo, &preset);
        } else {
            anyhow::bail!(
                "no env source found for {}/{} — cannot continue",
                pane.repo,
                preset
            );
        }
    }

    Ok(preset)
}

pub fn run_workspace(
    workspace: &str,
    preset_override: Option<&str>,
    backend: Option<&dyn SecretBackend>,
    settings: &Settings,
) -> Result<()> {
    let service = &settings.project.dev_ui.repo;
    let preset = resolve_workspace_preset(service, workspace, preset_override, backend, settings)?;
    println!("PRESET:{}", preset);
    Ok(())
}

pub fn run_backend(
    preset_override: Option<&str>,
    backend: Option<&dyn SecretBackend>,
    settings: &Settings,
) -> Result<()> {
    let preset = resolve_backend_preset(preset_override, backend, settings)?;
    println!("PRESET:{}", preset);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SecretBackend as SB, Settings};
    use crate::test_utils::ENV_LOCK;

    fn no_backend_settings() -> Settings {
        Settings {
            secret_backend: SB::None,
            op_vault: String::new(),
            session_mux: crate::config::SessionMultiplexer::Tmux,
            color_diff: true,
            project: Default::default(),
        }
    }

    #[test]
    fn prompt_preset_uses_last_over_available() {
        let last: Option<&str> = Some("uat");
        let available = ["dev".to_string(), "test".to_string(), "uat".to_string()];
        let default = last
            .or_else(|| available.first().map(|s| s.as_str()))
            .unwrap_or("test");
        assert_eq!(default, "uat");
    }

    #[test]
    fn prompt_preset_falls_back_to_first_available_when_no_last() {
        let last: Option<&str> = None;
        let available = ["dev".to_string(), "test".to_string()];
        let default = last
            .or_else(|| available.first().map(|s| s.as_str()))
            .unwrap_or("test");
        assert_eq!(default, "dev");
    }

    #[test]
    fn prompt_preset_falls_back_to_test_when_nothing() {
        let last: Option<&str> = None;
        let available: Vec<String> = vec![];
        let default = last
            .or_else(|| available.first().map(|s| s.as_str()))
            .unwrap_or("test");
        assert_eq!(default, "test");
    }

    #[test]
    fn load_service_for_pick_returns_skip_when_repo_missing() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().unwrap();
        crate::test_utils::set_repo_root(root.path().to_str().unwrap());

        let dest = root.path().join("nonexistent-dir").join(".env");
        let settings = no_backend_settings();
        let (ok, msg) = load_service_for_pick("svc-a", "test", &dest, None, &settings);

        crate::test_utils::clear_repo_root();

        assert!(!ok);
        assert!(msg.contains("not found"));
    }
}
