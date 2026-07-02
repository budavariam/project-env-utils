/// .state/active.json — tracks which preset each service/workspace is on.
///
/// State file format:
/// ```json
/// {
///   "workspaces": {"/path/to/workspace": "preset-name"},
///   "services": {"my-api": "test", "my-ui": "uat"}
/// }
/// ```
/// Writes are atomic (temp file + rename).
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::state_file;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct State {
    #[serde(default)]
    pub workspaces: HashMap<String, String>,
    #[serde(default)]
    pub services: HashMap<String, String>,
}

impl State {
    pub fn load() -> Self {
        load_from(&state_file())
    }

    #[allow(dead_code)]
    pub fn load_from(path: &Path) -> Self {
        load_from(path)
    }

    /// Atomically write state to the default state file.
    pub fn save(&self) -> Result<()> {
        save_to(self, &state_file())
    }

    /// Atomically write state to a custom path (used in tests).
    #[allow(dead_code)]
    pub fn save_to(&self, path: &Path) -> Result<()> {
        save_to(self, path)
    }

    pub fn get_workspace_preset(&self, workspace: &str) -> Option<&str> {
        self.workspaces.get(workspace).map(|s| s.as_str())
    }

    pub fn set_workspace_preset(&mut self, workspace: &str, preset: &str) {
        self.workspaces
            .insert(workspace.to_string(), preset.to_string());
    }

    pub fn get_service_preset(&self, service: &str) -> Option<&str> {
        self.services.get(service).map(|s| s.as_str())
    }

    pub fn set_service_preset(&mut self, service: &str, preset: &str) {
        self.services
            .insert(service.to_string(), preset.to_string());
    }

    pub fn remove_workspace(&mut self, workspace: &str) {
        self.workspaces.remove(workspace);
    }

    pub fn remove_service(&mut self, service: &str) {
        self.services.remove(service);
    }
}

fn load_from(path: &Path) -> State {
    if !path.exists() {
        return State::default();
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_to(state: &State, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(state)? + "\n";
    // Atomic write: temp file in same directory, then rename.
    let tmp_path = temp_path_for(path);
    std::fs::write(&tmp_path, &data)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

fn temp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("active.json");
    path.parent()
        .unwrap_or(Path::new("."))
        .join(format!("{}.tmp", file_name))
}

// ── Convenience functions that operate on the default state file ───────────────
//
// Each of these does a full disk round-trip (load → mutate → save). Callers
// that need to perform multiple updates should use `State::load` / `State::save`
// directly to avoid redundant I/O.

pub fn get_workspace_preset(workspace: &str) -> Option<String> {
    State::load()
        .get_workspace_preset(workspace)
        .map(|s| s.to_string())
}

pub fn set_workspace_preset(workspace: &str, preset: &str) -> Result<()> {
    let mut state = State::load();
    state.set_workspace_preset(workspace, preset);
    state.save()
}

pub fn get_service_preset(service: &str) -> Option<String> {
    State::load()
        .get_service_preset(service)
        .map(|s| s.to_string())
}

pub fn set_service_preset(service: &str, preset: &str) -> Result<()> {
    let mut state = State::load();
    state.set_service_preset(service, preset);
    state.save()
}

pub fn all_tracked_workspaces() -> HashMap<String, String> {
    State::load().workspaces
}

pub fn all_tracked_services() -> HashMap<String, String> {
    State::load().services
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn missing_state_file_returns_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let state = State::load_from(&path);
        assert!(state.workspaces.is_empty());
        assert!(state.services.is_empty());
    }

    #[test]
    fn round_trip_services() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("active.json");

        let mut state = State::default();
        state.set_service_preset("my-api", "test");
        state.set_service_preset("my-ui", "uat");
        state.save_to(&path).unwrap();

        let loaded = State::load_from(&path);
        assert_eq!(loaded.get_service_preset("my-api"), Some("test"));
        assert_eq!(loaded.get_service_preset("my-ui"), Some("uat"));
        assert_eq!(loaded.get_service_preset("missing"), None);
    }

    #[test]
    fn round_trip_workspaces() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("active.json");

        let mut state = State::default();
        state.set_workspace_preset("/home/user/project/my-worktree", "dev");
        state.save_to(&path).unwrap();

        let loaded = State::load_from(&path);
        assert_eq!(
            loaded.get_workspace_preset("/home/user/project/my-worktree"),
            Some("dev")
        );
    }

    #[test]
    fn atomic_write_uses_tmp_then_rename() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("active.json");

        let mut state = State::default();
        state.set_service_preset("my-api", "test");
        state.save_to(&path).unwrap();

        // Temp file should be gone after successful write
        let tmp = dir.path().join("active.json.tmp");
        assert!(!tmp.exists(), "tmp file should be cleaned up after rename");

        // The actual file should exist
        assert!(path.exists());
    }

    #[test]
    fn remove_workspace_and_service() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("active.json");

        let mut state = State::default();
        state.set_service_preset("my-api", "test");
        state.set_workspace_preset("/ws/a", "dev");
        state.save_to(&path).unwrap();

        let mut loaded = State::load_from(&path);
        loaded.remove_service("my-api");
        loaded.remove_workspace("/ws/a");
        loaded.save_to(&path).unwrap();

        let final_state = State::load_from(&path);
        assert!(final_state.services.is_empty());
        assert!(final_state.workspaces.is_empty());
    }

    #[test]
    fn update_existing_preset() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("active.json");

        let mut state = State::default();
        state.set_service_preset("my-api", "test");
        state.save_to(&path).unwrap();

        let mut loaded = State::load_from(&path);
        loaded.set_service_preset("my-api", "uat");
        loaded.save_to(&path).unwrap();

        let final_state = State::load_from(&path);
        assert_eq!(final_state.get_service_preset("my-api"), Some("uat"));
    }
}
