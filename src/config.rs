//! Shared configuration for penv.
//!
//! Project config is read from `settings.json` (tracked in git).
//! Personal config is read from `settings.local.json` (gitignored).
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

// ── Project config structs ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct SecretFileConfig {
    /// Short identifier used as the storage key and display name.
    /// Must be unique within the service.
    pub label: String,
    /// Path to the file relative to the service repo root (e.g. "keys/my.key").
    pub path: String,
    /// When true the file content is the same across all presets and is stored
    /// once (not per-preset). When false (default) a separate copy is kept per preset.
    #[serde(default)]
    pub shared: bool,
}

impl SecretFileConfig {
    /// The key used to identify this file in a secret backend.
    ///
    /// Shared files use just the label; preset-specific files use `"<preset>/<label>"`.
    pub fn backend_key(&self, preset: &str) -> String {
        if self.shared {
            self.label.clone()
        } else {
            format!("{}/{}", preset, self.label)
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServiceConfig {
    pub name: String,
    /// Short human-readable description stored as the item note in 1Password.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub env_vars: Vec<String>,
    /// Path to .env file relative to the service repo root.
    /// Defaults to ".env" when absent.
    #[serde(default)]
    pub env_path: Option<String>,
    /// Additional sensitive files managed alongside the .env (e.g. key files).
    #[serde(default)]
    pub files: Vec<SecretFileConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SessionPaneConfig {
    pub repo: String,
    pub cmd: String,
}

/// One pane in a flexible grid layout — at most 2 columns, unlimited rows.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct GridPaneConfig {
    /// Repo folder name relative to `repo_parent()`. Empty = working dir stays at repo_parent().
    #[serde(default)]
    pub repo: String,
    /// Command to run in the pane. Empty = idle shell.
    #[serde(default)]
    pub cmd: String,
    /// 0-based column index (0 or 1 — capped at 1).
    #[serde(default)]
    pub col: usize,
    /// 0-based row index.
    #[serde(default)]
    pub row: usize,
}

/// One tmux window (tab) within a session.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TabConfig {
    /// Window name shown in the tmux status bar.
    #[serde(default)]
    pub name: String,
    /// Panes to place in the grid. Gaps become idle shells automatically.
    #[serde(default)]
    pub panes: Vec<GridPaneConfig>,
}

/// A fully described tmux session using the flexible grid layout.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SessionConfig {
    pub session_name: String,
    /// Skip preset resolution and env sync — for sessions that don't use secrets.
    #[serde(default)]
    pub no_preset: bool,
    /// Custom message shown in the infobox (word-wrapped). Displayed above the notes section.
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub tabs: Vec<TabConfig>,
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

/// Config for a linked window from an external tmux session.
/// When enabled, the named external session's window is linked into the dev
/// session as a new tab — the view stays live even if the dev session restarts.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LinkedWindowConfig {
    #[serde(default)]
    pub enabled: bool,
    /// External tmux session to link from. Defaults to `"claude"`.
    #[serde(default)]
    pub session_name: String,
    /// Window name or index within the external session to link.
    /// Defaults to `session_name` when empty.
    #[serde(default)]
    pub window: String,
    /// Command to run when the external session is auto-created (first launch).
    /// Defaults to `session_name` when empty.
    #[serde(default)]
    pub cmd: String,
    /// Starting directory for auto-created sessions (supports `~`).
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
    pub claude: LinkedWindowConfig,
    /// Flexible grid sessions (used by `penv dev-session`).
    pub sessions: Vec<SessionConfig>,
}

// ── Runtime paths ──────────────────────────────────────────────────────────────

/// Directory that contains the penv settings (= the repo root).
///
/// Resolution order:
///   1. `PENV_REPO_ROOT` env var (set by `--config` flag or manually) — always re-checked
///   2. Current directory — if it contains a valid `settings.json`  (cached after first detection)
///   3. Git root of the current directory — if it contains a valid `settings.json`
///   4. Directory containing the penv binary (original fallback)
static DETECTED_ROOT: OnceLock<PathBuf> = OnceLock::new();

pub fn repo_root() -> PathBuf {
    // Always honour the env var — tests and --config flag rely on this taking effect
    // immediately even after the OnceLock is populated.
    if let Ok(override_path) = std::env::var("PENV_REPO_ROOT") {
        return PathBuf::from(override_path);
    }
    // Cache the expensive detection (spawns `git rev-parse`) after first call.
    DETECTED_ROOT
        .get_or_init(|| {
            if let Some(found) = detect_settings_dir() {
                return found;
            }
            std::env::current_exe()
                .ok()
                .and_then(|p| std::fs::canonicalize(&p).ok())
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| PathBuf::from("."))
        })
        .clone()
}

/// Check the current directory then its git root for a valid penv `settings.json`.
fn detect_settings_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    if is_penv_root(&cwd) {
        return Some(cwd.clone());
    }
    let git_root = git_root_of(&cwd)?;
    if git_root != cwd && is_penv_root(&git_root) {
        return Some(git_root);
    }
    None
}

/// Returns true if `dir/settings.json` looks like a penv config.
///
/// Requires both a non-empty `project.name` **and** the presence of a
/// `services` array.  The second check prevents false positives from
/// unrelated tools (npm, build systems) whose `settings.json` may also
/// carry a `project.name` field.
fn is_penv_root(dir: &std::path::Path) -> bool {
    std::fs::read_to_string(dir.join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| {
            let has_name = v
                .get("project")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            let has_services = v.get("services").and_then(|s| s.as_array()).is_some();
            has_name && has_services
        })
        .unwrap_or(false)
}

/// Run `git rev-parse --show-toplevel` in `dir` and return the result.
fn git_root_of(dir: &std::path::Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .ok()?;
    if out.status.success() {
        String::from_utf8(out.stdout)
            .ok()
            .map(|s| PathBuf::from(s.trim()))
    } else {
        None
    }
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

impl ProjectConfig {
    /// Resolve the `.env` path for a service, respecting the per-service `env_path` override
    /// in settings.json. Falls back to `<repo_parent>/<service>/.env` when unset.
    /// The `workspace` parameter takes precedence (used for UI worktrees).
    pub fn service_env_path(&self, service: &str, workspace: Option<&str>) -> PathBuf {
        if let Some(ws) = workspace {
            return PathBuf::from(ws).join(".env");
        }
        let custom = self
            .services
            .iter()
            .find(|s| s.name == service)
            .and_then(|s| s.env_path.as_deref());
        repo_parent().join(service).join(custom.unwrap_or(".env"))
    }

    /// `local/<project_name>/<service>/<preset>.env` — preset-specific local fallback (gitignored).
    pub fn preset_local_path(&self, service: &str, preset: &str) -> std::path::PathBuf {
        repo_root()
            .join("local")
            .join(&self.project_name)
            .join(service)
            .join(format!("{}.env", preset))
    }

    /// `local/<project_name>/<service>/local.env` — generic local fallback (gitignored, has secrets).
    pub fn local_fallback_path(&self, service: &str) -> std::path::PathBuf {
        repo_root()
            .join("local")
            .join(&self.project_name)
            .join(service)
            .join("local.env")
    }

    /// Relative display path for the preset local cache (for user-facing messages).
    pub fn preset_local_rel(&self, service: &str, preset: &str) -> String {
        format!("local/{}/{}/{}.env", self.project_name, service, preset)
    }

    /// Relative display path for the generic local fallback (for user-facing messages).
    pub fn local_fallback_rel(&self, service: &str) -> String {
        format!("local/{}/{}/local.env", self.project_name, service)
    }

    /// Destination path for a managed file inside the service repo.
    pub fn service_file_dest(&self, service: &str, file_cfg: &SecretFileConfig) -> PathBuf {
        repo_parent().join(service).join(&file_cfg.path)
    }

    /// Local cache path for a managed file (gitignored).
    ///
    /// Shared files:         `local/<project>/shared/<label>`
    /// Service-specific:     `local/<project>/<service>/<label>`
    /// Preset-specific:      `local/<project>/<service>/<preset>.<label>`
    pub fn file_local_cache(
        &self,
        service: &str,
        file_cfg: &SecretFileConfig,
        preset: &str,
    ) -> PathBuf {
        if file_cfg.shared {
            repo_root()
                .join("local")
                .join(&self.project_name)
                .join("shared")
                .join(&file_cfg.label)
        } else {
            let name = format!("{}.{}", preset, file_cfg.label);
            repo_root()
                .join("local")
                .join(&self.project_name)
                .join(service)
                .join(name)
        }
    }
}

/// `env/<service>/<preset>.env` — tracked preset profile (no secrets).
pub fn fallback_profile_path(service: &str, preset: &str) -> PathBuf {
    repo_root()
        .join("env")
        .join(service)
        .join(format!("{}.env", preset))
}

/// List available preset names for a service from `local/<project>/<service>/`.
/// Returns preset names (filenames without .env extension), excluding the generic "local" fallback.
pub fn available_presets(service: &str) -> Vec<String> {
    let project_name = Settings::load()
        .map(|s| s.project.project_name.clone())
        .unwrap_or_default();
    if project_name.is_empty() {
        return Vec::new();
    }
    available_presets_for_project(service, &project_name)
}

/// List available preset names for a service from `local/<project>/<service>/`.
pub fn available_presets_for_project(service: &str, project_name: &str) -> Vec<String> {
    let dir = repo_root().join("local").join(project_name).join(service);
    let mut presets = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("env")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && stem != "local"
            {
                presets.push(stem.to_string());
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
    Sqlite,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionMultiplexer {
    Tmux,
    Byobu,
    Screen,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub secret_backend: SecretBackend,
    /// 1Password vault name — local override takes precedence over project default.
    pub op_vault: String,
    pub session_mux: SessionMultiplexer,
    /// Whether to use ANSI colors in diff output. Default true; disable via "diff_color": false
    /// in settings.local.json, --no-color flag, NO_COLOR env var, or non-TTY stdout.
    pub color_diff: bool,
    pub project: ProjectConfig,
}

impl Settings {
    pub fn load() -> anyhow::Result<Self> {
        let config_file = project_config_file();
        if !config_file.exists() {
            anyhow::bail!(
                "settings.json not found at {:?}\nRun `penv init` to create one.",
                config_file
            );
        }

        let local = load_json(&settings_file());
        let proj_json = load_json(&config_file);

        let backend = match local.get("secret_backend").and_then(|v| v.as_str()) {
            Some("onepassword") => SecretBackend::OnePassword,
            Some("none") => SecretBackend::None,
            _ => SecretBackend::Sqlite, // default when unset or "sqlite"
        };

        let project = parse_project(&proj_json);

        if project.project_name.is_empty() {
            anyhow::bail!(
                "settings.json must have a non-empty project.name field.\n\
                 Example: {{\"project\": {{\"name\": \"my-project\", ...}}}}"
            );
        }

        let op_vault = local
            .get("onepassword_vault")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| project.op_vault.clone());

        let session_mux = match local.get("session_multiplexer").and_then(|v| v.as_str()) {
            Some("byobu") => SessionMultiplexer::Byobu,
            Some("screen") => SessionMultiplexer::Screen,
            _ => SessionMultiplexer::Tmux, // default
        };

        let color_diff = local
            .get("diff_color")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        Ok(Settings {
            secret_backend: backend,
            op_vault,
            session_mux,
            color_diff,
            project,
        })
    }

    pub fn backend_label(&self) -> &'static str {
        match self.secret_backend {
            SecretBackend::OnePassword => "1Password",
            SecretBackend::Sqlite => "SQLite",
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
        claude: LinkedWindowConfig,
        #[serde(default)]
        sessions: Vec<SessionConfig>,
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
        sessions: raw.sessions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::ENV_LOCK;

    #[test]
    fn available_presets_returns_sorted_stems() {
        use std::fs;
        let base = tempfile::tempdir().unwrap();
        let repo_root_dir = base.path().join("tool");
        fs::create_dir_all(&repo_root_dir).unwrap();
        let local_dir = repo_root_dir
            .join("local")
            .join("MyProject")
            .join("my-service");
        fs::create_dir_all(&local_dir).unwrap();
        fs::write(local_dir.join("uat.env"), "").unwrap();
        fs::write(local_dir.join("test.env"), "").unwrap();
        fs::write(local_dir.join("dev.env"), "").unwrap();
        fs::write(local_dir.join("local.env"), "").unwrap(); // generic fallback — excluded
        fs::write(local_dir.join("README.md"), "").unwrap(); // non-.env — excluded

        crate::test_utils::set_repo_root(repo_root_dir.to_str().unwrap());
        let presets = available_presets_for_project("my-service", "MyProject");
        crate::test_utils::clear_repo_root();

        assert_eq!(presets, vec!["dev", "test", "uat"]);
    }

    #[test]
    fn settings_onepassword_vault_override() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        // settings.json with required project.name
        let mut pf = std::fs::File::create(dir.path().join("settings.json")).unwrap();
        write!(pf, r#"{{"project":{{"name":"TestProj"}}}}"#).unwrap();
        // settings.local.json with backend override
        let mut lf = std::fs::File::create(dir.path().join("settings.local.json")).unwrap();
        write!(
            lf,
            r#"{{"secret_backend": "onepassword", "onepassword_vault": "My Vault"}}"#
        )
        .unwrap();

        crate::test_utils::set_repo_root(dir.path().to_str().unwrap());
        let s = Settings::load().unwrap();
        crate::test_utils::clear_repo_root();

        assert_eq!(s.secret_backend, SecretBackend::OnePassword);
        assert_eq!(s.op_vault, "My Vault");
    }

    #[test]
    fn settings_no_file_returns_error() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        crate::test_utils::set_repo_root(dir.path().to_str().unwrap());
        let result = Settings::load();
        crate::test_utils::clear_repo_root();
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("settings.json not found"), "got: {}", msg);
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

        crate::test_utils::set_repo_root(dir.path().to_str().unwrap());
        let s = Settings::load().unwrap();
        crate::test_utils::clear_repo_root();

        assert_eq!(s.project.project_name, "Acme");
        assert_eq!(s.project.bucket, "acme-secrets");
        assert_eq!(s.op_vault, "Acme Dev");
        assert_eq!(s.project.services.len(), 1);
        assert_eq!(s.project.services[0].name, "svc-a");
        assert_eq!(s.project.services[0].env_vars, vec!["FOO", "BAR"]);
        assert_eq!(s.project.services[0].files, vec![]);
        assert_eq!(s.project.dev_ui.repo, "svc-a");
        assert_eq!(s.project.dev_backend.session_name, "acme-backend");
    }

    #[test]
    fn repo_root_detects_cwd_with_settings() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"project":{"name":"DetectMe"}}"#,
        )
        .unwrap();

        crate::test_utils::set_repo_root(dir.path().to_str().unwrap());
        let root = repo_root();
        crate::test_utils::clear_repo_root();

        assert_eq!(root, dir.path());
    }

    #[test]
    fn is_penv_root_returns_false_for_empty_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"project":{"name":""}}"#,
        )
        .unwrap();
        assert!(!is_penv_root(dir.path()));
    }

    #[test]
    fn is_penv_root_returns_false_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_penv_root(dir.path()));
    }

    #[test]
    fn is_penv_root_returns_true_for_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"project":{"name":"MyProj"},"services":[]}"#,
        )
        .unwrap();
        assert!(is_penv_root(dir.path()));
    }
}
