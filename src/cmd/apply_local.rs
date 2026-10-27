/// `penv apply-local [service...]`
///
/// Treat the local cache (`local/<project>/<service>/<preset>.env`) as the source of truth
/// for each service's active preset: push it to the backend if it differs, then load it
/// into the service repo.
use anyhow::Result;

use crate::backend::SecretBackend;
use crate::cmd::validate_cache::timestamp;
use crate::config::{Settings, backups_dir};
use crate::state::get_service_preset;

pub fn run(services: &[String], backend: &dyn SecretBackend, settings: &Settings) -> Result<()> {
    let services: Vec<&str> = if services.is_empty() {
        settings.project.services.iter().map(|s| s.name.as_str()).collect()
    } else {
        services.iter().map(String::as_str).collect()
    };

    for svc in services {
        let Some(preset) = get_service_preset(svc) else {
            eprintln!("  skip {} — no active preset (run change_preset)", svc);
            continue;
        };
        let local_path = settings.project.preset_local_path(svc, &preset);
        let Ok(local) = std::fs::read_to_string(&local_path) else {
            eprintln!("  skip {}/{} — no local cache file", svc, preset);
            continue;
        };

        match backend.fetch(svc, &preset) {
            Some(remote) if remote == local => {}
            remote => {
                if let Some(remote) = remote {
                    let dir = backups_dir().join(format!("{}-apply-local", timestamp()));
                    std::fs::create_dir_all(&dir).ok();
                    let _ = std::fs::write(dir.join(format!("{}.{}.before.env", svc, preset)), remote);
                }
                if !backend.push(svc, &preset, &local) {
                    eprintln!("  ✗ {}/{}: push to {} failed, not loading", svc, preset, backend.label());
                    continue;
                }
                println!("  ↑ {}/{}: pushed to {}", svc, preset, backend.label());
            }
        }

        crate::cmd::load_env::load_service(svc, &preset, Some(backend), settings);
    }
    Ok(())
}
