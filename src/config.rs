//! Shared configuration for penv.
//!
//! Project config is read from `settings.json` (tracked in git).
//! Personal config is read from `settings.local.json` (gitignored).
use std::path::PathBuf;

use serde::Deserialize;

// ── Project config structs ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServiceConfig {
    pub name: String,
    /// Short human-readable description stored as the item note in 1Password.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub env_vars: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SessionPaneConfig {
    pub repo: String,
    pub cmd: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DevUiConfig {
    pub repo: String,
    pub pane_dev_cmd: String,
    pub pane_sb_cmd: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DevBackendConfig {
    pub session_name: String,
    #[serde(default)]
    pub window_backend: Vec<SessionPaneConfig>,
    #[serde(default)]
    pub window_service: Vec<SessionPaneConfig>,
}

/// Optional Claude session attached to a dev session.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ClaudeSessionConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Starting directory for the Claude tmux session (supports `~`).
    #[serde(default)]
    pub start_dir: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectConfig {
    pub project_name: String,
    pub bucket: String,
    pub op_vault: String,
    /// Prefix prepended to every 1Password item title, e.g. "myproj" → items named "myproj/my-api".
    /// Leave empty to use bare service names.
    pub op_item_prefix: String,
    pub services: Vec<ServiceConfig>,
    pub dev_ui: DevUiConfig,
    pub dev_backend: DevBackendConfig,
    pub claude: ClaudeSessionConfig,
}

// ── Runtime paths ──────────────────────────────────────────────────────────────

/// Directory that contains the penv binary (= the repo root).
pub fn repo_root() -> PathBuf {
    if let Ok(override_path) = std::env::var("PENV_REPO_ROOT") {
        return PathBuf::from(override_path);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(&p).ok())
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Parent of repo_root — the directory that holds all service repos.
pub fn repo_parent() -> PathBuf {
    repo_root()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".."))
}

pub fn backups_dir() -> PathBuf {
    repo_root().join("backups")
}

pub fn state_file() -> PathBuf {
    repo_root().join(".state").join("active.json")
}

/// Personal/local settings (gitignored).
pub fn settings_file() -> PathBuf {
    repo_root().join("settings.local.json")
}

/// Project settings (tracked in git).
pub fn project_config_file() -> PathBuf {
    repo_root().join("settings.json")
}

// ── File path helpers ──────────────────────────────────────────────────────────

/// Destination .env path for a service (or workspace).
/// Service repos live in repo_parent(); workspace paths are absolute.
pub fn local_env_path(service: &str, workspace: Option<&str>) -> PathBuf {
    if let Some(ws) = workspace {
        PathBuf::from(ws).join(".env")
    } else {
        repo_parent().join(service).join(".env")
    }
}

/// `local/<service>.<preset>.env` — preset-specific local fallback (gitignored).
/// Lives inside the tool directory so all secrets stay in one place.
pub fn preset_local_path(service: &str, preset: &str) -> PathBuf {
    repo_root()
        .join("local")
        .join(format!("{}.{}.env", service, preset))
}

/// `local/<service>.env` — generic local fallback (gitignored, has secrets).
pub fn local_fallback_path(service: &str) -> PathBuf {
    repo_root().join("local").join(format!("{}.env", service))
}

/// `env/<service>/<preset>.env` — tracked preset profile (no secrets).
pub fn fallback_profile_path(service: &str, preset: &str) -> PathBuf {
    repo_root()
        .join("env")
        .join(service)
        .join(format!("{}.env", preset))
}

/// List available preset names for a service from `env/<service>/`.
pub fn available_presets(service: &str) -> Vec<String> {
    let dir = repo_root().join("env").join(service);
    let mut presets = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("env") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    presets.push(stem.to_string());
                }
            }
        }
    }
    presets.sort();
    presets
}

// ── Settings ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum SecretBackend {
    OnePassword,
    None,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub secret_backend: SecretBackend,
    /// 1Password vault name — local override takes precedence over project default.
    pub op_vault: String,
    pub project: ProjectConfig,
}

impl Settings {
    pub fn load() -> Self {
        let local = load_json(&settings_file());
        let proj_json = load_json(&project_config_file());

        let backend = {
            if let Some(b) = local.get("secret_backend").and_then(|v| v.as_str()) {
                match b {
                    "onepassword" => SecretBackend::OnePassword,
                    _ => SecretBackend::None,
                }
} else {
                SecretBackend::None
            }
        };

        let project = parse_project(&proj_json);

        let op_vault = local
            .get("onepassword_vault")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| project.op_vault.clone());


        Settings {
            secret_backend: backend,
            op_vault,
            project,
        }
    }

    pub fn backend_label(&self) -> &'static str {
        match self.secret_backend {
            SecretBackend::OnePassword => "1Password",
            SecretBackend::None => "local-only",
        }
    }
}

fn load_json(path: &std::path::Path) -> serde_json::Value {
    if path.exists() {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Object(Default::default()))
    } else {
        serde_json::Value::Object(Default::default())
    }
}

fn parse_project(json: &serde_json::Value) -> ProjectConfig {
    // Parse via serde for nested structs.
    #[derive(Deserialize, Default)]
    struct Raw {
        #[serde(default)]
        project: RawProject,
        #[serde(default)]
        services: Vec<ServiceConfig>,
        #[serde(default)]
        dev_ui: DevUiConfig,
        #[serde(default)]
        dev_backend: DevBackendConfig,
        #[serde(default)]
        claude: ClaudeSessionConfig,
    }
    #[derive(Deserialize, Default)]
    struct RawProject {
        #[serde(default)]
        name: String,
        #[serde(default)]
        bucket: String,
        #[serde(default)]
        op_vault: String,
        #[serde(default)]
        op_item_prefix: String,
    }

    let raw: Raw = serde_json::from_value(json.clone()).unwrap_or_default();
    ProjectConfig {
        project_name: raw.project.name,
        bucket: raw.project.bucket,
        op_vault: raw.project.op_vault,
        op_item_prefix: raw.project.op_item_prefix,
        services: raw.services,
        dev_ui: raw.dev_ui,
        dev_backend: raw.dev_backend,
        claude: raw.claude,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::ENV_LOCK;

    #[test]
    fn available_presets_returns_sorted_stems() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use std::fs;
        let base = tempfile::tempdir().unwrap();
        // repo_root = base/tool, env/ now lives inside repo_root
        let repo_root_dir = base.path().join("tool");
        fs::create_dir_all(&repo_root_dir).unwrap();
        let env_dir = repo_root_dir.join("env").join("my-service");
        fs::create_dir_all(&env_dir).unwrap();
        fs::write(env_dir.join("uat.env"), "").unwrap();
        fs::write(env_dir.join("test.env"), "").unwrap();
        fs::write(env_dir.join("dev.env"), "").unwrap();
        fs::write(env_dir.join("README.md"), "").unwrap();

        std::env::set_var("PENV_REPO_ROOT", repo_root_dir.to_str().unwrap());
        let presets = available_presets("my-service");
        std::env::remove_var("PENV_REPO_ROOT");

        assert_eq!(presets, vec!["dev", "test", "uat"]);
    }


    #[test]
    fn settings_onepassword_vault_override() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.local.json");
        let mut f = std::fs::File::create(&settings_path).unwrap();
        write!(
            f,
            r#"{{"secret_backend": "onepassword", "onepassword_vault": "My Vault"}}"#
        )
        .unwrap();

        std::env::set_var("PENV_REPO_ROOT", dir.path().to_str().unwrap());
        let s = Settings::load();
        std::env::remove_var("PENV_REPO_ROOT");

        assert_eq!(s.secret_backend, SecretBackend::OnePassword);
        assert_eq!(s.op_vault, "My Vault");
    }

    #[test]
    fn settings_no_file_returns_local_only() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("PENV_REPO_ROOT", dir.path().to_str().unwrap());
        let s = Settings::load();
        std::env::remove_var("PENV_REPO_ROOT");
        assert_eq!(s.secret_backend, SecretBackend::None);
        assert!(s.op_vault.is_empty());
    }

    #[test]
    fn settings_project_config_loaded() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("settings.json")).unwrap();
        write!(
            f,
            r#"{{"project":{{"name":"Acme","bucket":"acme-secrets","op_vault":"Acme Dev"}},"services":[{{"name":"svc-a","env_vars":["FOO","BAR"]}}],"dev_ui":{{"repo":"svc-a","pane_dev_cmd":"npm run dev","pane_sb_cmd":"npm run sb"}},"dev_backend":{{"session_name":"acme-backend","window_backend":[],"window_service":[]}}}}"#
        )
        .unwrap();

        std::env::set_var("PENV_REPO_ROOT", dir.path().to_str().unwrap());
        let s = Settings::load();
        std::env::remove_var("PENV_REPO_ROOT");

        assert_eq!(s.project.project_name, "Acme");
        assert_eq!(s.project.bucket, "acme-secrets");
        assert_eq!(s.op_vault, "Acme Dev");
        assert_eq!(s.project.services.len(), 1);
        assert_eq!(s.project.services[0].name, "svc-a");
        assert_eq!(s.project.services[0].env_vars, vec!["FOO", "BAR"]);
        assert_eq!(s.project.dev_ui.repo, "svc-a");
        assert_eq!(s.project.dev_backend.session_name, "acme-backend");
    }
}
