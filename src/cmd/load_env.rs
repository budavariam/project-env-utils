/// `penv load-env <preset> [service]`
///
/// Load a preset's .env into service repos.
/// Source priority:
///   1. Secret backend (1Password)
///   2. local/<service>.<preset>.env  (preset-specific local)
///   3. local/<service>.env           (generic local fallback)
///   4. env/<service>/<preset>.env    (git-tracked profile, no secrets)
use anyhow::Result;

use crate::backend::SecretBackend;
use crate::config::{SecretFileConfig, Settings, fallback_profile_path};
use crate::env_file::write_file;
use crate::state::set_service_preset;

pub fn run(preset: &str, service_filter: Option<&str>, settings: &Settings) -> Result<()> {
    let services: Vec<&str> = if let Some(svc) = service_filter {
        vec![svc]
    } else {
        settings
            .project
            .services
            .iter()
            .map(|s| s.name.as_str())
            .collect()
    };

    println!("Loading preset: {}", preset);

    let backend = crate::backend::active_backend(settings);

    for svc in &services {
        if load_service(svc, preset, backend.as_deref(), settings) {
            let _ = set_service_preset(svc, preset);
        }
    }
    println!("Done.");
    Ok(())
}

/// Returns true if the service was successfully loaded.
pub fn load_service(
    service: &str,
    preset: &str,
    backend: Option<&dyn SecretBackend>,
    settings: &Settings,
) -> bool {
    let dest = settings.project.service_env_path(service, None);
    if !dest.parent().map(|p| p.exists()).unwrap_or(false) {
        println!(
            "  skip {} — repo not found at {:?}",
            service,
            dest.parent().unwrap_or(&dest)
        );
        return false;
    }

    let env_ok = load_service_env(service, preset, backend, settings, &dest);

    // Load managed files regardless of whether .env loading succeeded.
    let files = settings
        .project
        .services
        .iter()
        .find(|s| s.name == service)
        .map(|s| s.files.as_slice())
        .unwrap_or(&[]);
    for file_cfg in files {
        load_service_file(service, preset, file_cfg, backend, settings);
    }

    env_ok
}

/// Inner helper: load the .env for `service`. Returns true on success.
fn load_service_env(
    service: &str,
    preset: &str,
    backend: Option<&dyn SecretBackend>,
    settings: &Settings,
    dest: &std::path::Path,
) -> bool {
    // 1. Secret backend
    if let Some(b) = backend {
        if let Some(content) = b.fetch(service, preset) {
            crate::backup::backup_env_before_write(
                &format!("{}/{}", service, preset),
                dest,
                "load-env",
                &format!("load-env {}/{} from {}", service, preset, b.label()),
            );
            if let Err(e) = write_file(dest, &content) {
                eprintln!("  error writing {}: {}", service, e);
                return false;
            }
            println!(
                "  wrote {}/.env  ({}: {})",
                service,
                settings.backend_label(),
                b.key_display(service, preset)
            );
            return true;
        }
        eprintln!(
            "  warn: '{}' not in {}, trying local",
            b.key_display(service, preset),
            settings.backend_label()
        );
    }

    // 2. Preset-specific local file
    let preset_local = settings.project.preset_local_path(service, preset);
    if preset_local.exists()
        && let Ok(content) = std::fs::read_to_string(&preset_local)
    {
        crate::backup::backup_env_before_write(
            &format!("{}/{}", service, preset),
            dest,
            "load-env",
            &format!(
                "load-env {}/{} from local cache {}",
                service,
                preset,
                settings.project.preset_local_rel(service, preset)
            ),
        );
        if let Err(e) = write_file(dest, &content) {
            eprintln!("  error writing {}: {}", service, e);
            return false;
        }
        println!("  wrote {}/.env  ({} local)", service, preset);
        return true;
    }

    // 3. Generic local fallback
    let fallback = settings.project.local_fallback_path(service);
    if fallback.exists()
        && let Ok(content) = std::fs::read_to_string(&fallback)
    {
        crate::backup::backup_env_before_write(
            &format!("{}/{}", service, preset),
            dest,
            "load-env",
            &format!(
                "load-env {}/{} from generic local fallback {}",
                service,
                preset,
                settings.project.local_fallback_rel(service)
            ),
        );
        if let Err(e) = write_file(dest, &content) {
            eprintln!("  error writing {}: {}", service, e);
            return false;
        }
        println!(
            "  wrote {}/.env  (local fallback — DB config may not match preset '{}')",
            service, preset
        );
        return true;
    }

    // 4. Git-tracked profile
    let profile = fallback_profile_path(service, preset);
    if profile.exists()
        && let Ok(content) = std::fs::read_to_string(&profile)
    {
        crate::backup::backup_env_before_write(
            &format!("{}/{}", service, preset),
            dest,
            "load-env",
            &format!(
                "load-env {}/{} from git profile env/{}/{}.env",
                service, preset, service, preset
            ),
        );
        if let Err(e) = write_file(dest, &content) {
            eprintln!("  error writing {}: {}", service, e);
            return false;
        }
        println!(
            "  wrote {}/.env  (profile only — no secrets, see docs/onepassword-setup.md)",
            service
        );
        return true;
    }

    println!(
        "  skip {} — no source found for preset '{}'",
        service, preset
    );
    false
}

/// Load a single managed file for a service, trying backend then local cache.
fn load_service_file(
    service: &str,
    preset: &str,
    file_cfg: &SecretFileConfig,
    backend: Option<&dyn SecretBackend>,
    settings: &Settings,
) {
    let dest = settings.project.service_file_dest(service, file_cfg);
    let key = file_cfg.backend_key(preset);

    // 1. Secret backend
    if let Some(b) = backend
        && let Some(content) = b.fetch_file(service, &key)
    {
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        crate::backup::backup_env_before_write(
            &format!("{}/{}", service, file_cfg.label),
            &dest,
            "load-env",
            &format!(
                "load-env {}/{} file {} from {}",
                service,
                preset,
                key,
                b.label()
            ),
        );
        if let Err(e) = write_file(&dest, &content) {
            eprintln!("  error writing {}/{}: {}", service, file_cfg.label, e);
        } else {
            println!(
                "  wrote {}/{}  ({}: {})",
                service,
                file_cfg.path,
                b.label(),
                key
            );
        }
        return;
    }

    // 2. Local file cache
    let cache = settings.project.file_local_cache(service, file_cfg, preset);
    if cache.exists()
        && let Ok(content) = std::fs::read_to_string(&cache)
    {
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        crate::backup::backup_env_before_write(
            &format!("{}/{}", service, file_cfg.label),
            &dest,
            "load-env",
            &format!(
                "load-env {}/{} file {} from local cache",
                service, preset, key
            ),
        );
        if let Err(e) = write_file(&dest, &content) {
            eprintln!("  error writing {}/{}: {}", service, file_cfg.label, e);
        } else {
            println!("  wrote {}/{}  (local cache)", service, file_cfg.path);
        }
        return;
    }

    println!(
        "  skip {}/{} — no source found (key: {})",
        service, file_cfg.label, key
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SecretBackend as SB, Settings};
    use crate::test_utils::ENV_LOCK;
    use tempfile::tempdir;

    struct FakeBackend {
        content: Option<String>,
        label: &'static str,
    }

    impl SecretBackend for FakeBackend {
        fn label(&self) -> &'static str {
            self.label
        }
        fn available(&self) -> bool {
            true
        }
        fn fetch(&self, _service: &str, _preset: &str) -> Option<String> {
            self.content.clone()
        }
        fn exists(&self, _service: &str, _preset: &str) -> bool {
            self.content.is_some()
        }
        fn push(&self, _service: &str, _preset: &str, _content: &str) -> bool {
            true
        }
        fn delete(&self, _service: &str, _preset: &str) -> bool {
            true
        }
        fn list(&self) -> Vec<(String, String)> {
            vec![]
        }
    }

    fn test_settings() -> Settings {
        use crate::config::ProjectConfig;
        Settings {
            secret_backend: SB::None,
            op_vault: String::new(),
            session_mux: crate::config::SessionMultiplexer::Tmux,
            color_diff: true,
            project: ProjectConfig {
                project_name: "test-proj".to_string(),
                ..Default::default()
            },
        }
    }

    fn setup_fixture(
        service: &str,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let base = tempdir().unwrap();
        let parent = base.path().to_path_buf();
        let root = parent.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let svc_dir = parent.join(service);
        std::fs::create_dir_all(&svc_dir).unwrap();
        (base, root, parent, svc_dir)
    }

    #[test]
    fn load_env_from_backend_takes_priority() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let service = "my-api";
        let (_base, root, _parent, svc_dir) = setup_fixture(service);

        let local_dir = root.join("local").join("test-proj");
        std::fs::create_dir_all(&local_dir).unwrap();
        std::fs::write(local_dir.join("my-api.env"), "FALLBACK=true\n").unwrap();

        crate::test_utils::set_repo_root(root.to_str().unwrap());

        let backend = FakeBackend {
            content: Some("FROM_BACKEND=1\n".to_string()),
            label: "1Password",
        };
        let settings = test_settings();
        let result = load_service(service, "test", Some(&backend), &settings);

        crate::test_utils::clear_repo_root();

        assert!(result, "load_service should succeed");
        let written = std::fs::read_to_string(svc_dir.join(".env")).unwrap();
        assert_eq!(written, "FROM_BACKEND=1\n");
    }

    #[test]
    fn load_env_falls_back_to_preset_local() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let service = "my-api";
        let (_base, root, _parent, svc_dir) = setup_fixture(service);

        let local_dir = root.join("local").join("test-proj").join(service);
        std::fs::create_dir_all(&local_dir).unwrap();
        std::fs::write(local_dir.join("test.env"), "PRESET_LOCAL=1\n").unwrap();

        crate::test_utils::set_repo_root(root.to_str().unwrap());
        let settings = test_settings();
        let result = load_service(service, "test", None, &settings);
        crate::test_utils::clear_repo_root();

        assert!(result);
        assert_eq!(
            std::fs::read_to_string(svc_dir.join(".env")).unwrap(),
            "PRESET_LOCAL=1\n"
        );
    }

    #[test]
    fn load_env_falls_back_to_generic_local() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let service = "my-api";
        let (_base, root, _parent, svc_dir) = setup_fixture(service);

        let local_dir = root.join("local").join("test-proj").join(service);
        std::fs::create_dir_all(&local_dir).unwrap();
        std::fs::write(local_dir.join("local.env"), "GENERIC_LOCAL=1\n").unwrap();

        crate::test_utils::set_repo_root(root.to_str().unwrap());
        let settings = test_settings();
        let result = load_service(service, "test", None, &settings);
        crate::test_utils::clear_repo_root();

        assert!(result);
        assert_eq!(
            std::fs::read_to_string(svc_dir.join(".env")).unwrap(),
            "GENERIC_LOCAL=1\n"
        );
    }

    #[test]
    fn load_env_falls_back_to_profile() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let service = "my-api";
        let (_base, root, _parent, svc_dir) = setup_fixture(service);

        let profile_dir = root.join("env").join(service);
        std::fs::create_dir_all(&profile_dir).unwrap();
        std::fs::write(profile_dir.join("test.env"), "PROFILE=1\n").unwrap();

        crate::test_utils::set_repo_root(root.to_str().unwrap());
        let settings = test_settings();
        let result = load_service(service, "test", None, &settings);
        crate::test_utils::clear_repo_root();

        assert!(result);
        assert_eq!(
            std::fs::read_to_string(svc_dir.join(".env")).unwrap(),
            "PROFILE=1\n"
        );
    }

    #[test]
    fn load_env_skips_when_no_source() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let service = "my-api";
        let (_base, root, _parent, _svc_dir) = setup_fixture(service);
        crate::test_utils::set_repo_root(root.to_str().unwrap());
        let result = load_service(service, "test", None, &test_settings());
        crate::test_utils::clear_repo_root();
        assert!(!result);
    }

    #[test]
    fn load_env_skips_when_repo_not_found() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = tempdir().unwrap();
        let root = base.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        crate::test_utils::set_repo_root(root.to_str().unwrap());
        let result = load_service("my-api", "test", None, &test_settings());
        crate::test_utils::clear_repo_root();
        assert!(!result);
    }

    #[test]
    fn backend_miss_then_falls_back_to_profile() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let service = "my-api";
        let (_base, root, _parent, svc_dir) = setup_fixture(service);

        let profile_dir = root.join("env").join(service);
        std::fs::create_dir_all(&profile_dir).unwrap();
        std::fs::write(profile_dir.join("test.env"), "FROM_PROFILE=1\n").unwrap();

        crate::test_utils::set_repo_root(root.to_str().unwrap());

        let backend = FakeBackend {
            content: None,
            label: "1Password",
        };
        let settings = Settings {
            secret_backend: SB::None,
            op_vault: String::new(),
            session_mux: crate::config::SessionMultiplexer::Tmux,
            color_diff: true,
            project: Default::default(),
        };
        let result = load_service(service, "test", Some(&backend), &settings);

        crate::test_utils::clear_repo_root();

        assert!(result);
        assert_eq!(
            std::fs::read_to_string(svc_dir.join(".env")).unwrap(),
            "FROM_PROFILE=1\n"
        );
    }
}
