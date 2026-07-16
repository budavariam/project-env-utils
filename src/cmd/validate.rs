//! `penv validate`
//!
//! Validates `settings.json` against its declared `$schema` path (resolved
//! relative to settings.json), falling back to `settings.schema.json` in
//! `repo_root()` when no `$schema` field is present.
use anyhow::{Context, Result, bail};
use std::path::PathBuf;

use crate::config::{project_config_file, repo_root};

pub fn run() -> Result<()> {
    let settings_path = project_config_file();

    let settings_raw = std::fs::read_to_string(&settings_path)
        .with_context(|| format!("cannot read {}", settings_path.display()))?;

    let data: serde_json::Value = serde_json::from_str(&settings_raw)
        .with_context(|| format!("{} is not valid JSON", settings_path.display()))?;

    // Resolve the schema path from the $schema field, relative to settings.json.
    let schema_path: PathBuf = if let Some(rel) = data.get("$schema").and_then(|v| v.as_str()) {
        let settings_dir = settings_path.parent().unwrap_or_else(|| std::path::Path::new("."));
        settings_dir.join(rel)
    } else {
        repo_root().join("settings.schema.json")
    };

    let schema_raw = std::fs::read_to_string(&schema_path).with_context(|| {
        format!(
            "cannot read schema at {} — check the $schema field in settings.json",
            schema_path.display()
        )
    })?;
    let schema: serde_json::Value =
        serde_json::from_str(&schema_raw).context("schema file is not valid JSON")?;

    let validator =
        jsonschema::validator_for(&schema).context("failed to compile schema")?;

    let errors: Vec<String> = validator
        .iter_errors(&data)
        .map(|e| format!("{} (at {})", e, e.instance_path()))
        .collect();

    if errors.is_empty() {
        println!("settings.json is valid.");
        Ok(())
    } else {
        for e in &errors {
            eprintln!("  ✗ {}", e);
        }
        bail!("{} validation error(s) found in settings.json", errors.len());
    }
}
