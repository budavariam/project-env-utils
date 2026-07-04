//! Shared sync logic: diff, push, pull, list — used by op_sync.
use crate::backend::SecretBackend;
use crate::config::{Settings};
use crate::env_file::read_file;
use similar::{ChangeTag, TextDiff};

/// Generate a unified diff string between `old` and `new` text.
/// Returns up to `max_lines` diff lines; appends a truncation note if more exist.
pub fn unified_diff(
    old: &str,
    new: &str,
    from_label: &str,
    to_label: &str,
    max_lines: usize,
) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut lines: Vec<String> = Vec::new();

    lines.push(format!("--- {}", from_label));
    lines.push(format!("+++ {}", to_label));

    for group in diff.grouped_ops(3) {
        for op in &group {
            for change in diff.iter_changes(op) {
                let prefix = match change.tag() {
                    ChangeTag::Delete => "-",
                    ChangeTag::Insert => "+",
                    ChangeTag::Equal => " ",
                };
                let line = change.value();
                lines.push(format!("{}{}", prefix, line.trim_end_matches('\n')));
            }
        }
    }

    let total = lines.len();
    if total > max_lines {
        let mut result = lines[..max_lines].join("\n");
        result.push_str(&format!("\n  ... {} more lines", total - max_lines));
        result
    } else {
        lines.join("\n")
    }
}

pub fn cmd_push(
    preset: &str,
    service_filter: Option<&str>,
    backend: &dyn SecretBackend,
    settings: &Settings,
    dry_run: bool,
) {
    let services = resolve_services(service_filter, settings);
    for service in &services {
        let path = settings.project.service_env_path(service, None);
        let content = match read_file(&path) {
            Some(c) => c,
            None => {
                println!("  skip {} — {:?} not found", service, path);
                continue;
            }
        };
        if dry_run {
            let action = if backend.exists(service, preset) { "UPDATE" } else { "CREATE" };
            println!(
                "  [dry-run] {} {}  ({} bytes)",
                action,
                backend.key_display(service, preset),
                content.len()
            );
        } else if backend.push(service, preset, &content) {
            println!("  pushed  {}", backend.key_display(service, preset));
            backend.post_push(service, preset);
        } else {
            eprintln!("  FAILED  {}", backend.key_display(service, preset));
        }
    }
    if dry_run {
        println!("Done (dry-run — no changes made).");
    } else {
        println!("Done.");
    }
}

pub fn cmd_pull(
    preset: &str,
    service_filter: Option<&str>,
    backend: &dyn SecretBackend,
    settings: &Settings,
    dry_run: bool,
) {
    let services = resolve_services(service_filter, settings);
    for service in &services {
        let dest = settings.project.service_env_path(service, None);
        match backend.fetch(service, preset) {
            None => println!(
                "  skip {} — '{}' not found in {}",
                service,
                backend.key_display(service, preset),
                backend.label()
            ),
            Some(content) => {
                if dry_run {
                    println!(
                        "  [dry-run] would write {} bytes to {:?}  (from {})",
                        content.len(),
                        dest,
                        backend.key_display(service, preset)
                    );
                } else {
                    if let Some(parent) = dest.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    crate::backup::backup_env_before_write(
                        service, &dest, "op-pull",
                        &format!("op pull {} from {}", preset, backend.key_display(service, preset)),
                    );
                    if std::fs::write(&dest, &content).is_ok() {
                        println!(
                            "  wrote {}/.env  (from {})",
                            service,
                            backend.key_display(service, preset)
                        );
                    } else {
                        eprintln!("  error writing {:?}", dest);
                    }
                }
            }
        }
    }
    if dry_run {
        println!("Done (dry-run — no changes made).");
    } else {
        println!("Done.");
    }
}

pub fn cmd_diff(
    preset: &str,
    service_filter: Option<&str>,
    backend: &dyn SecretBackend,
    settings: &Settings,
) {
    let services = resolve_services(service_filter, settings);
    let mut any_diff = false;

    for service in &services {
        let local = read_file(&settings.project.service_env_path(service, None));
        let remote = backend.fetch(service, preset);

        match (local.as_deref(), remote.as_deref()) {
            (None, None) => {
                println!("  {}: not found locally or in {}", service, backend.label());
            }
            (Some(_), None) => {
                println!(
                    "  {}: local .env exists, not in {} ({})",
                    service,
                    backend.label(),
                    backend.key_display(service, preset)
                );
                any_diff = true;
            }
            (None, Some(_)) => {
                println!(
                    "  {}: {} has {}, local .env missing",
                    service,
                    backend.label(),
                    backend.key_display(service, preset)
                );
                any_diff = true;
            }
            (Some(loc), Some(rem)) => {
                if loc == rem {
                    println!("  {}: in sync ✓", service);
                } else {
                    any_diff = true;
                    let from_label = format!(
                        "{}/{}",
                        backend.label().to_lowercase(),
                        backend.key_display(service, preset)
                    );
                    let to_label = format!("local/{}/.env", service);
                    let diff_output = unified_diff(rem, loc, &from_label, &to_label, 40);
                    println!("\n{} differs:", service);
                    println!("{}", diff_output);
                }
            }
        }
    }

    if !any_diff {
        println!("All services are in sync with {}.", backend.label());
    }
}

pub fn cmd_list(service_filter: Option<&str>, backend: &dyn SecretBackend, settings: &Settings) {
    let known_services: Vec<&str> = settings
        .project
        .services
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    let all_pairs = backend.list();

    let mut shown: Vec<&(String, String)> = if let Some(svc) = service_filter {
        all_pairs.iter().filter(|(s, _)| s == svc).collect()
    } else {
        all_pairs
            .iter()
            .filter(|(s, _)| known_services.contains(&s.as_str()))
            .collect()
    };
    shown.sort();

    if shown.is_empty() {
        let label = if let Some(svc) = service_filter {
            format!("for {}", svc)
        } else {
            format!("in {}", settings.project.bucket)
        };
        println!("No secrets found {}.", label);
        return;
    }

    use std::collections::BTreeMap;
    let mut grouped: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (svc, preset) in &shown {
        grouped.entry(svc.as_str()).or_default().push(preset.as_str());
    }
    for (svc, mut presets) in grouped {
        presets.sort();
        println!("  {}:  {}", svc, presets.join("  "));
    }
}

fn resolve_services(filter: Option<&str>, settings: &Settings) -> Vec<String> {
    if let Some(svc) = filter {
        let known: Vec<&str> = settings
            .project
            .services
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        if !known.contains(&svc) {
            eprintln!("warn: '{}' is not a known service; proceeding anyway", svc);
        }
        vec![svc.to_string()]
    } else {
        settings
            .project
            .services
            .iter()
            .map(|s| s.name.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProjectConfig, SecretBackend as SB, ServiceConfig, Settings};
    use crate::test_utils::ENV_LOCK;
    use std::sync::{Arc, Mutex};

    // ── unified_diff tests ─────────────────────────────────────────────────────

    #[test]
    fn unified_diff_truncates_at_max_lines() {
        let old: String = (0..50).map(|i| format!("line{}\n", i)).collect();
        let new: String = (50..100).map(|i| format!("line{}\n", i)).collect();
        let diff = unified_diff(&old, &new, "old", "new", 40);
        let line_count = diff.lines().count();
        assert!(diff.contains("more lines"), "should mention truncation");
        assert!(line_count <= 42, "got {} lines", line_count);
    }

    #[test]
    fn unified_diff_identical_produces_minimal_output() {
        let text = "FOO=bar\nBAZ=qux\n";
        let diff = unified_diff(text, text, "a", "b", 40);
        let change_lines: Vec<&str> = diff
            .lines()
            .filter(|l| {
                (l.starts_with('+') || l.starts_with('-'))
                    && !l.starts_with("+++")
                    && !l.starts_with("---")
            })
            .collect();
        assert!(change_lines.is_empty(), "identical files should have no change lines");
    }

    #[test]
    fn unified_diff_shows_changes() {
        let old = "FOO=bar\n";
        let new = "FOO=baz\n";
        let diff = unified_diff(old, new, "old", "new", 40);
        assert!(diff.contains("-FOO=bar"));
        assert!(diff.contains("+FOO=baz"));
    }

    // ── RecordingBackend — fake for cmd_* tests ────────────────────────────────

    #[derive(Default)]
    struct RecordingBackend {
        /// Pairs that exist: (service, preset) → content
        store: Mutex<std::collections::HashMap<(String, String), String>>,
        push_log: Mutex<Vec<(String, String)>>,
    }

    impl RecordingBackend {
        fn with_entry(service: &str, preset: &str, content: &str) -> Arc<Self> {
            let b = Arc::new(Self::default());
            b.store
                .lock()
                .unwrap()
                .insert((service.to_string(), preset.to_string()), content.to_string());
            b
        }
    }

    impl SecretBackend for RecordingBackend {
        fn label(&self) -> &'static str {
            "Test"
        }
        fn available(&self) -> bool {
            true
        }
        fn fetch(&self, service: &str, preset: &str) -> Option<String> {
            self.store
                .lock()
                .unwrap()
                .get(&(service.to_string(), preset.to_string()))
                .cloned()
        }
        fn exists(&self, service: &str, preset: &str) -> bool {
            self.store
                .lock()
                .unwrap()
                .contains_key(&(service.to_string(), preset.to_string()))
        }
        fn push(&self, service: &str, preset: &str, content: &str) -> bool {
            self.push_log
                .lock()
                .unwrap()
                .push((service.to_string(), preset.to_string()));
            self.store
                .lock()
                .unwrap()
                .insert((service.to_string(), preset.to_string()), content.to_string());
            true
        }
        fn delete(&self, service: &str, preset: &str) -> bool {
            self.store
                .lock()
                .unwrap()
                .remove(&(service.to_string(), preset.to_string()))
                .is_some()
        }
        fn list(&self) -> Vec<(String, String)> {
            self.store.lock().unwrap().keys().cloned().collect()
        }
    }

    fn test_settings_with_services(services: &[&str]) -> Settings {
        Settings {
            secret_backend: SB::None,
            op_vault: String::new(),
            session_mux: crate::config::SessionMultiplexer::Tmux,
            project: ProjectConfig {
                services: services
                    .iter()
                    .map(|s| ServiceConfig { name: s.to_string(), description: String::new(), env_vars: vec![], env_path: None })
                    .collect(),
                ..Default::default()
            },
        }
    }

    fn setup_fs(base: &std::path::Path, service: &str, content: &str) {
        let svc_dir = base.join(service);
        std::fs::create_dir_all(&svc_dir).unwrap();
        std::fs::write(svc_dir.join(".env"), content).unwrap();
    }

    // ── cmd_pull tests ─────────────────────────────────────────────────────────

    #[test]
    fn cmd_pull_writes_file_when_secret_exists() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("my-project");
        let svc_dir = dir.path().join("my-api");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&svc_dir).unwrap();

        std::env::set_var("PENV_REPO_ROOT", root.to_str().unwrap());

        let backend = RecordingBackend::with_entry("my-api", "test", "SECRET=pulled\n");
        let settings = test_settings_with_services(&["my-api"]);
        cmd_pull("test", None, backend.as_ref(), &settings, false);

        std::env::remove_var("PENV_REPO_ROOT");

        let content = std::fs::read_to_string(svc_dir.join(".env")).unwrap();
        assert_eq!(content, "SECRET=pulled\n");
    }

    #[test]
    fn cmd_pull_dry_run_does_not_write_file() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("my-project");
        let svc_dir = dir.path().join("my-api");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&svc_dir).unwrap();

        std::env::set_var("PENV_REPO_ROOT", root.to_str().unwrap());

        let backend = RecordingBackend::with_entry("my-api", "test", "SECRET=pulled\n");
        let settings = test_settings_with_services(&["my-api"]);
        cmd_pull("test", None, backend.as_ref(), &settings, true);

        std::env::remove_var("PENV_REPO_ROOT");

        assert!(!svc_dir.join(".env").exists(), "dry-run must not write the file");
    }

    #[test]
    fn cmd_pull_skips_when_secret_missing() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("my-project");
        let svc_dir = dir.path().join("my-api");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&svc_dir).unwrap();

        std::env::set_var("PENV_REPO_ROOT", root.to_str().unwrap());

        let backend = Arc::new(RecordingBackend::default()); // empty store
        let settings = test_settings_with_services(&["my-api"]);
        cmd_pull("test", None, backend.as_ref(), &settings, false);

        std::env::remove_var("PENV_REPO_ROOT");

        assert!(!svc_dir.join(".env").exists());
    }

    // ── cmd_push tests ─────────────────────────────────────────────────────────

    #[test]
    fn cmd_push_calls_backend_for_each_service() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("my-project");
        std::fs::create_dir_all(&root).unwrap();
        setup_fs(dir.path(), "my-api", "A=1\n");
        setup_fs(dir.path(), "my-ui", "B=2\n");

        std::env::set_var("PENV_REPO_ROOT", root.to_str().unwrap());

        let backend = Arc::new(RecordingBackend::default());
        let settings = test_settings_with_services(&["my-api", "my-ui"]);
        cmd_push("test", None, backend.as_ref(), &settings, false);

        std::env::remove_var("PENV_REPO_ROOT");

        let log = backend.push_log.lock().unwrap();
        assert_eq!(log.len(), 2);
        assert!(log.contains(&("my-api".to_string(), "test".to_string())));
        assert!(log.contains(&("my-ui".to_string(), "test".to_string())));
    }

    #[test]
    fn cmd_push_dry_run_does_not_call_backend() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("my-project");
        std::fs::create_dir_all(&root).unwrap();
        setup_fs(dir.path(), "my-api", "A=1\n");

        std::env::set_var("PENV_REPO_ROOT", root.to_str().unwrap());

        let backend = Arc::new(RecordingBackend::default());
        let settings = test_settings_with_services(&["my-api"]);
        cmd_push("test", None, backend.as_ref(), &settings, true);

        std::env::remove_var("PENV_REPO_ROOT");

        let log = backend.push_log.lock().unwrap();
        assert!(log.is_empty(), "dry-run must not call backend.push()");
    }

    #[test]
    fn cmd_push_skips_service_without_local_env() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("my-project");
        std::fs::create_dir_all(&root).unwrap();
        // my-api exists, my-ui does NOT have a .env
        setup_fs(dir.path(), "my-api", "A=1\n");
        std::fs::create_dir_all(dir.path().join("my-ui")).unwrap();

        std::env::set_var("PENV_REPO_ROOT", root.to_str().unwrap());

        let backend = Arc::new(RecordingBackend::default());
        let settings = test_settings_with_services(&["my-api", "my-ui"]);
        cmd_push("test", None, backend.as_ref(), &settings, false);

        std::env::remove_var("PENV_REPO_ROOT");

        let log = backend.push_log.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0], ("my-api".to_string(), "test".to_string()));
    }
}
