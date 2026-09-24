use std::collections::HashSet;

use crate::config::Settings;

/// List available preset names, one per line.
///
/// Without a service argument: prints presets present for ALL configured
/// services (intersection) — safe to pass to `load-env` without specifying a service.
/// With a service argument: lists presets for that service only.
pub fn run(service: Option<&str>, settings: &Settings) -> anyhow::Result<()> {
    let presets = if let Some(svc) = service {
        settings.project.available_presets_for_service(svc)
    } else {
        let services = &settings.project.services;
        if services.is_empty() {
            return Ok(());
        }
        let mut common: HashSet<String> = settings
            .project
            .available_presets_for_service(&services[0].name)
            .into_iter()
            .collect();
        for svc in services.iter().skip(1) {
            let svc_presets: HashSet<String> = settings
                .project
                .available_presets_for_service(&svc.name)
                .into_iter()
                .collect();
            common = common.intersection(&svc_presets).cloned().collect();
        }
        let mut sorted: Vec<String> = common.into_iter().collect();
        sorted.sort();
        sorted
    };

    for preset in presets {
        println!("{}", preset);
    }
    Ok(())
}
