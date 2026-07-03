pub mod onepassword;

use anyhow::Result;

use crate::config::{SecretBackend as SB, Settings};

/// Common trait implemented by both secret backends.
pub trait SecretBackend: Send + Sync {
    /// Check whether this backend is reachable and authenticated.
    fn available(&self) -> bool;

    /// Fetch env content for (service, preset). Returns None if not found or on error.
    fn fetch(&self, service: &str, preset: &str) -> Option<String>;

    /// Check whether (service, preset) exists in the backend.
    fn exists(&self, service: &str, preset: &str) -> bool;

    /// Create or update env content for (service, preset). Returns true on success.
    fn push(&self, service: &str, preset: &str, content: &str) -> bool;

    /// Delete a (service, preset) pair. Returns true on success.
    #[allow(dead_code)]
    fn delete(&self, service: &str, preset: &str) -> bool;

    /// List all (service, preset) pairs visible through this backend.
    fn list(&self) -> Vec<(String, String)>;

    /// Human-readable backend label.
    fn label(&self) -> &'static str;

    /// Human-readable key description shown in logs (e.g. "my-ui:test").
    /// Backends may override to show their internal key format.
    fn key_display(&self, service: &str, preset: &str) -> String {
        format!("{}:{}", service, preset)
    }

    /// Hook called after a successful push (e.g., bucket assignment). Default is a no-op.
    fn post_push(&self, _service: &str, _preset: &str) {}
}

/// Run an external command, returning (success, stdout, stderr).
pub fn run_cmd(args: &[&str]) -> (bool, String, String) {
    if args.is_empty() {
        return (false, String::new(), "no command provided".to_string());
    }
    let result = std::process::Command::new(args[0])
        .args(&args[1..])
        .output();
    match result {
        Ok(out) => {
            let success = out.status.success();
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            (success, stdout, stderr)
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("No such file or directory") || msg.contains("not found") {
                (
                    false,
                    String::new(),
                    format!("{} not found in PATH", args[0]),
                )
            } else {
                (false, String::new(), msg)
            }
        }
    }
}

/// Write content to a temporary .env file. Caller is responsible for deleting.
#[allow(dead_code)]
pub fn write_tempfile(content: &str) -> Result<std::path::PathBuf> {
    write_tempfile_ext(content, ".env")
}

/// Write content to a temporary .json file. Caller is responsible for deleting.
pub fn write_tempfile_json(content: &str) -> Result<std::path::PathBuf> {
    write_tempfile_ext(content, ".json")
}

fn write_tempfile_ext(content: &str, ext: &str) -> Result<std::path::PathBuf> {
    let mut file = tempfile::Builder::new().suffix(ext).tempfile()?;
    use std::io::Write;
    file.write_all(content.as_bytes())?;
    let (_, path) = file.keep()?;
    Ok(path)
}

/// Instantiate the configured secret backend and return it if reachable.
pub fn active_backend(settings: &Settings) -> Option<Box<dyn SecretBackend>> {
    match &settings.secret_backend {
        SB::OnePassword => {
            let descriptions: std::collections::HashMap<String, String> = settings
                .project.services.iter()
                .filter(|s| !s.description.is_empty())
                .map(|s| (s.name.clone(), s.description.clone()))
                .collect();
            let b = onepassword::OpBackend::new(
                &settings.op_vault,
                &settings.project.op_item_prefix,
            ).with_descriptions(descriptions);
            if b.available() {
                Some(Box::new(b))
            } else {
                None
            }
        }
        SB::None => None,
    }
}
