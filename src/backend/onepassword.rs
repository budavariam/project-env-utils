/// 1Password backend — wraps `op` CLI subprocess calls.
///
/// Storage model: one Secure Note item per service, one section per preset,
/// with a single "env" field in each section containing the full .env content.
///
/// Example vault layout:
///   Item "my-ui"   → section "test" → field "env" = "..."
///                   → section "uat"  → field "env" = "..."
///   Item "my-api"  → section "test" → field "env" = "..."

use super::{run_cmd, write_tempfile_json, SecretBackend};

pub struct OpBackend {
    pub vault: String,
    /// Prefix prepended to item titles, e.g. "myproj" → "myproj/my-api".
    pub item_prefix: String,
    /// Optional per-service descriptions stored as item notes.
    pub descriptions: std::collections::HashMap<String, String>,
}

impl OpBackend {
    pub fn new(vault: impl Into<String>, item_prefix: impl Into<String>) -> Self {
        OpBackend {
            vault: vault.into(),
            item_prefix: item_prefix.into(),
            descriptions: std::collections::HashMap::new(),
        }
    }

    pub fn with_descriptions(mut self, descriptions: std::collections::HashMap<String, String>) -> Self {
        self.descriptions = descriptions;
        self
    }

    /// Return the 1Password item title for a service.
    fn item_title(&self, service: &str) -> String {
        if self.item_prefix.is_empty() {
            service.to_string()
        } else {
            format!("{}/{}", self.item_prefix, service)
        }
    }

    fn run(&self, args: &[&str]) -> (bool, String, String) {
        run_cmd(args)
    }

    pub fn vault_exists(&self) -> bool {
        let (ok, _, _) = self.run(&["op", "vault", "get", &self.vault]);
        ok
    }

    pub fn init_vault(&self, services: &[String]) -> anyhow::Result<()> {
        println!("Checking vault '{}'...", self.vault);
        if self.vault_exists() {
            println!("  '{}' exists.", self.vault);
        } else {
            println!("  '{}' not found.", self.vault);
            print!("  Create it? [y/N]: ");
            use std::io::{self, BufRead, Write};
            io::stdout().flush()?;
            let stdin = io::stdin();
            let answer = stdin
                .lock()
                .lines()
                .next()
                .and_then(|l| l.ok())
                .map(|l| l.trim().to_lowercase())
                .unwrap_or_default();

            if answer == "y" {
                let (ok, _, err) = self.run(&["op", "vault", "create", &self.vault]);
                if ok {
                    println!("  Created '{}'.", self.vault);
                } else {
                    anyhow::bail!("failed to create vault '{}': {}", self.vault, err.trim());
                }
            } else {
                println!("  Aborted.");
                return Ok(());
            }
        }

        println!("Done.");
        Ok(())
    }

    #[allow(dead_code)]
    fn item_exists(&self, title: &str) -> bool {
        let (ok, _, _) = self.run(&[
            "op", "item", "get", title,
            "--vault", &self.vault,
            "--fields", "title",
        ]);
        ok
    }

    fn get_item_json(&self, service: &str) -> Option<serde_json::Value> {
        self.get_item_by_title(&self.item_title(service))
    }

    fn get_item_by_title(&self, title: &str) -> Option<serde_json::Value> {
        let (ok, stdout, _) = self.run(&[
            "op", "item", "get", title,
            "--vault", &self.vault,
            "--format", "json",
        ]);
        if !ok {
            return None;
        }
        serde_json::from_str(&stdout).ok()
    }

    /// Find the "env" field value inside the section whose label == preset.
    fn extract_preset(item: &serde_json::Value, preset: &str) -> Option<String> {
        let fields = item.get("fields")?.as_array()?;
        for field in fields {
            let section_label = field
                .get("section")
                .and_then(|s| s.get("label"))
                .and_then(|v| v.as_str());
            let field_label = field.get("label").and_then(|v| v.as_str());
            if section_label == Some(preset) && field_label == Some("env") {
                return field.get("value").and_then(|v| v.as_str()).map(|s| s.to_string());
            }
        }
        None
    }

    /// Return a copy of `item` with the (preset, content) section+field upserted.
    fn upsert_preset_in_json(
        item: &serde_json::Value,
        preset: &str,
        content: &str,
    ) -> serde_json::Value {
        let mut sections: Vec<serde_json::Value> = item
            .get("sections")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut fields: Vec<serde_json::Value> = item
            .get("fields")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // Find or create the section for this preset.
        let section_id = sections
            .iter()
            .find(|s| s.get("label").and_then(|v| v.as_str()) == Some(preset))
            .and_then(|s| s.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| format!("s_{}", preset));

        if !sections
            .iter()
            .any(|s| s.get("label").and_then(|v| v.as_str()) == Some(preset))
        {
            sections.push(serde_json::json!({"id": section_id, "label": preset}));
        }

        // Find or create the env field in that section.
        let existing_idx = fields.iter().position(|f| {
            let sl = f.get("section").and_then(|s| s.get("label")).and_then(|v| v.as_str());
            let fl = f.get("label").and_then(|v| v.as_str());
            sl == Some(preset) && fl == Some("env")
        });
        let field_id = fields
            .get(existing_idx.unwrap_or(usize::MAX))
            .and_then(|f| f.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| format!("f_{}_env", preset));
        let new_field = serde_json::json!({
            "id": field_id,
            "type": "CONCEALED",
            "section": {"id": section_id, "label": preset},
            "label": "env",
            "value": content
        });
        if let Some(idx) = existing_idx {
            fields[idx] = new_field;
        } else {
            fields.push(new_field);
        }

        let mut updated = item.clone();
        updated["sections"] = serde_json::Value::Array(sections);
        updated["fields"] = serde_json::Value::Array(fields);
        updated
    }

    /// Return a copy of `item` with the section and field for `preset` removed.
    #[allow(dead_code)]
    fn remove_preset_from_json(item: &serde_json::Value, preset: &str) -> serde_json::Value {
        let section_id_to_remove: Option<String> = item
            .get("sections")
            .and_then(|v| v.as_array())
            .and_then(|secs| {
                secs.iter()
                    .find(|s| s.get("label").and_then(|v| v.as_str()) == Some(preset))
                    .and_then(|s| s.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
            });

        let sections: Vec<serde_json::Value> = item
            .get("sections")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.get("label").and_then(|v| v.as_str()) != Some(preset))
            .collect();

        let fields: Vec<serde_json::Value> = item
            .get("fields")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|f| {
                let sl = f.get("section").and_then(|s| s.get("label")).and_then(|v| v.as_str());
                let si = f.get("section").and_then(|s| s.get("id")).and_then(|v| v.as_str());
                let by_label = sl != Some(preset);
                let by_id = section_id_to_remove
                    .as_deref()
                    .map(|id| si != Some(id))
                    .unwrap_or(true);
                by_label && by_id
            })
            .collect();

        let mut updated = item.clone();
        updated["sections"] = serde_json::Value::Array(sections);
        updated["fields"] = serde_json::Value::Array(fields);
        updated
    }

    /// Strip server-assigned fields before using JSON as an `op item create --template`.
    fn clean_for_template(item: &serde_json::Value) -> serde_json::Value {
        let mut obj = match item.as_object() {
            Some(o) => o.clone(),
            None => return item.clone(),
        };
        for key in &["id", "vault", "version", "last_edited_by", "created_at", "updated_at"] {
            obj.remove(*key);
        }
        serde_json::Value::Object(obj)
    }

    /// Build a fresh item JSON for a brand-new service with a single preset.
    fn create_item_json(title: &str, preset: &str, content: &str, description: &str) -> serde_json::Value {
        let section_id = format!("s_{}", preset);
        let field_id = format!("f_{}_env", preset);
        let mut item = serde_json::json!({
            "title": title,
            "category": "SECURE_NOTE",
            "sections": [{"id": section_id, "label": preset}],
            "fields": [{
                "id": field_id,
                "type": "CONCEALED",
                "section": {"id": section_id, "label": preset},
                "label": "env",
                "value": content
            }]
        });
        if !description.is_empty() {
            item["notesPlain"] = serde_json::Value::String(description.to_string());
        }
        item
    }

    /// Write `template` to a temp file, run `op item create --template`, clean up.
    fn create_from_template(&self, template: &serde_json::Value) -> bool {
        let json_str = match serde_json::to_string(template) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  error serializing item template: {}", e);
                return false;
            }
        };
        let tmp = match write_tempfile_json(&json_str) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("  error creating temp file: {}", e);
                return false;
            }
        };
        let tmp_str = tmp.to_string_lossy().into_owned();
        let title = template
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let mut args = vec![
            "op", "item", "create",
            "--template", &tmp_str,
            "--vault", &self.vault,
        ];
        if !title.is_empty() {
            args.push("--title");
            args.push(&title);
        }
        let (ok, _, err) = self.run(&args);
        let _ = std::fs::remove_file(&tmp);
        if !ok {
            eprintln!("  op error: {}", err.trim());
        }
        ok
    }
}

impl SecretBackend for OpBackend {
    fn label(&self) -> &'static str {
        "1Password"
    }

    fn available(&self) -> bool {
        let (ok, _, stderr) = self.run(&["op", "whoami"]);
        if stderr.contains("not found in PATH") {
            return false;
        }
        ok
    }

    fn fetch(&self, service: &str, preset: &str) -> Option<String> {
        let item = self.get_item_json(service)?;
        Self::extract_preset(&item, preset)
    }

    fn exists(&self, service: &str, preset: &str) -> bool {
        self.fetch(service, preset).is_some()
    }

    fn push(&self, service: &str, preset: &str, content: &str) -> bool {
        let title = self.item_title(service);
        let description = self.descriptions.get(service).map(|s| s.as_str()).unwrap_or("");
        match self.get_item_json(service) {
            None => {
                let template = Self::create_item_json(&title, preset, content, description);
                self.create_from_template(&template)
            }
            Some(ref existing) => {
                let updated = Self::upsert_preset_in_json(existing, preset, content);
                let template = Self::clean_for_template(&updated);
                let (del_ok, _, err) =
                    self.run(&["op", "item", "delete", &title, "--vault", &self.vault]);
                if !del_ok {
                    eprintln!("  op error deleting for update: {}", err.trim());
                    return false;
                }
                self.create_from_template(&template)
            }
        }
    }

    fn delete(&self, service: &str, preset: &str) -> bool {
        let title = self.item_title(service);
        let item = match self.get_item_json(service) {
            Some(v) => v,
            None => {
                eprintln!("  item '{}' not found in 1Password", title);
                return false;
            }
        };

        let sections = item
            .get("sections")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let remaining = sections
            .iter()
            .filter(|s| s.get("label").and_then(|v| v.as_str()) != Some(preset))
            .count();

        if remaining == 0 {
            let (ok, _, err) =
                self.run(&["op", "item", "delete", &title, "--vault", &self.vault]);
            if !ok {
                eprintln!("  op error: {}", err.trim());
            }
            return ok;
        }

        // Other presets remain: delete item and recreate without this section.
        let updated = Self::remove_preset_from_json(&item, preset);
        let template = Self::clean_for_template(&updated);
        let (del_ok, _, err) =
            self.run(&["op", "item", "delete", &title, "--vault", &self.vault]);
        if !del_ok {
            eprintln!("  op error deleting item: {}", err.trim());
            return false;
        }
        self.create_from_template(&template)
    }

    fn list(&self) -> Vec<(String, String)> {
        let (ok, stdout, _) = self.run(&[
            "op", "item", "list",
            "--vault", &self.vault,
            "--categories", "Secure Note",
            "--format", "json",
        ]);
        if !ok {
            return vec![];
        }
        let items: Vec<serde_json::Value> = match serde_json::from_str(&stdout) {
            Ok(v) => v,
            Err(_) => return vec![],
        };

        let prefix_strip = if self.item_prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", self.item_prefix)
        };

        let mut pairs = vec![];
        for item in &items {
            let title = match item.get("title").and_then(|v| v.as_str()) {
                Some(t) if !t.is_empty() => t,
                _ => continue,
            };
            // Strip prefix to recover the bare service name used by the rest of the code.
            let service_name = if !prefix_strip.is_empty() && title.starts_with(&prefix_strip) {
                title[prefix_strip.len()..].to_string()
            } else {
                title.to_string()
            };
            if let Some(full) = self.get_item_by_title(title) {
                if let Some(sections) = full.get("sections").and_then(|v| v.as_array()) {
                    for section in sections {
                        if let Some(label) = section.get("label").and_then(|v| v.as_str()) {
                            pairs.push((service_name.clone(), label.to_string()));
                        }
                    }
                }
            }
        }
        pairs
    }
}

#[cfg(test)]
mod tests {
    use super::OpBackend;

    fn make_item(sections: &[(&str, &str)]) -> serde_json::Value {
        // sections: list of (preset_label, env_content)
        let mut secs = vec![];
        let mut fields = vec![];
        for (preset, content) in sections {
            let sid = format!("s_{}", preset);
            let fid = format!("f_{}_env", preset);
            secs.push(serde_json::json!({"id": sid, "label": preset}));
            fields.push(serde_json::json!({
                "id": fid,
                "type": "CONCEALED",
                "section": {"id": sid, "label": preset},
                "label": "env",
                "value": content
            }));
        }
        serde_json::json!({
            "id": "item-uuid",
            "title": "my-ui",
            "category": "SECURE_NOTE",
            "vault": {"id": "vault-uuid", "name": "Project Dev"},
            "sections": secs,
            "fields": fields
        })
    }

    #[test]
    fn extract_preset_finds_correct_section() {
        let item = make_item(&[("test", "API=test_val\n"), ("uat", "API=uat_val\n")]);
        assert_eq!(
            OpBackend::extract_preset(&item, "test"),
            Some("API=test_val\n".to_string())
        );
        assert_eq!(
            OpBackend::extract_preset(&item, "uat"),
            Some("API=uat_val\n".to_string())
        );
    }

    #[test]
    fn extract_preset_returns_none_for_missing() {
        let item = make_item(&[("test", "FOO=1\n")]);
        assert_eq!(OpBackend::extract_preset(&item, "dev"), None);
    }

    #[test]
    fn extract_preset_handles_multiline_content() {
        let content = "API_URL=https://example.com\nDB_PASS=s3cr3t\nFOO=bar\n";
        let item = make_item(&[("test", content)]);
        assert_eq!(OpBackend::extract_preset(&item, "test"), Some(content.to_string()));
    }

    #[test]
    fn upsert_preset_adds_new_section() {
        let item = make_item(&[("test", "FOO=1\n")]);
        let updated = OpBackend::upsert_preset_in_json(&item, "uat", "FOO=2\n");

        let sections = updated["sections"].as_array().unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(OpBackend::extract_preset(&updated, "uat"), Some("FOO=2\n".to_string()));
        // Existing preset untouched
        assert_eq!(OpBackend::extract_preset(&updated, "test"), Some("FOO=1\n".to_string()));
    }

    #[test]
    fn upsert_preset_updates_existing_section() {
        let item = make_item(&[("test", "OLD=value\n")]);
        let updated = OpBackend::upsert_preset_in_json(&item, "test", "NEW=value\n");

        let sections = updated["sections"].as_array().unwrap();
        assert_eq!(sections.len(), 1, "should not add a duplicate section");
        assert_eq!(
            OpBackend::extract_preset(&updated, "test"),
            Some("NEW=value\n".to_string())
        );
    }

    #[test]
    fn remove_preset_from_json_removes_section_and_field() {
        let item = make_item(&[("test", "T=1\n"), ("uat", "U=2\n")]);
        let updated = OpBackend::remove_preset_from_json(&item, "test");

        let sections = updated["sections"].as_array().unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0]["label"], "uat");
        assert_eq!(OpBackend::extract_preset(&updated, "test"), None);
        assert_eq!(OpBackend::extract_preset(&updated, "uat"), Some("U=2\n".to_string()));
    }

    #[test]
    fn clean_for_template_strips_server_fields() {
        let item = make_item(&[("test", "X=1\n")]);
        let cleaned = OpBackend::clean_for_template(&item);

        assert!(cleaned.get("id").is_none(), "id should be stripped");
        assert!(cleaned.get("vault").is_none(), "vault should be stripped");
        assert!(cleaned.get("title").is_some(), "title should be kept");
        assert!(cleaned.get("category").is_some(), "category should be kept");
        assert!(cleaned.get("sections").is_some(), "sections should be kept");
        assert!(cleaned.get("fields").is_some(), "fields should be kept");
    }

    #[test]
    fn create_item_json_has_correct_structure() {
        let item = OpBackend::create_item_json("my-api", "test", "KEY=val\n", "");

        assert_eq!(item["title"], "my-api");
        assert_eq!(item["category"], "SECURE_NOTE");

        let sections = item["sections"].as_array().unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0]["label"], "test");

        assert_eq!(
            OpBackend::extract_preset(&item, "test"),
            Some("KEY=val\n".to_string())
        );
    }
}
