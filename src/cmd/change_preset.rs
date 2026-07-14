/// `penv change-preset [service...]`
///
/// Interactively pick a new preset from local cache, then reload .env for
/// each given service (or all configured services when none are specified).
use std::io::{self, Write};

use anyhow::Result;

use crate::config::{Settings, available_presets_for_project};

pub fn run(services: &[String], settings: &Settings) -> Result<()> {
    let project_name = &settings.project.project_name;

    // Collect available presets — union across all given services (or all
    // configured services) so the list is complete regardless of which service
    // directory we probe first.
    let probe: Vec<String> = if services.is_empty() {
        settings
            .project
            .services
            .iter()
            .map(|s| s.name.clone())
            .collect()
    } else {
        services.to_vec()
    };

    let mut presets: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for svc in &probe {
            for p in available_presets_for_project(svc, project_name) {
                if seen.insert(p.clone()) {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    };

    // Also include any presets already in state but not in local files.
    {
        let state = crate::state::State::load();
        for p in state.services.values() {
            if !presets.contains(p) {
                presets.push(p.clone());
            }
        }
        presets.sort();
    }

    if presets.is_empty() {
        eprintln!("No presets found under local/{}/", project_name);
        return Ok(());
    }

    // Interactive menu.
    eprintln!("Available presets:");
    for (i, p) in presets.iter().enumerate() {
        eprintln!("  [{}] {}", i + 1, p);
    }

    let choice = loop {
        eprint!("Select preset [1-{}]: ", presets.len());
        let _ = io::stderr().flush();
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let trimmed = line.trim();
        if let Ok(n) = trimmed.parse::<usize>()
            && n >= 1
            && n <= presets.len()
        {
            break presets[n - 1].clone();
        }
        // Also accept typing the preset name directly.
        if presets.contains(&trimmed.to_string()) {
            break trimmed.to_string();
        }
        eprintln!("  Invalid choice — enter a number or preset name.");
    };

    eprintln!("Switching to preset '{}'...", choice);

    if services.is_empty() {
        crate::cmd::load_env::run(&choice, None, settings)?;
    } else {
        for svc in services {
            crate::cmd::load_env::run(&choice, Some(svc.as_str()), settings)?;
        }
    }

    Ok(())
}
