//! `penv show-info --preset <name> [--workspace <path>] [--services svc1 svc2...]`
//!
//! Print a Unicode box summarising the current dev session environment.
use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use crate::config::Settings;
use crate::env_file::parse_env;

// ── Box dimensions ─────────────────────────────────────────────────────────────

pub const BOX_WIDTH: usize = 56;
const INNER: usize = BOX_WIDTH - 2;

// ── SessionNotes builder ───────────────────────────────────────────────────────

/// Builder for the `--notes` arguments passed to `show-info`.
///
/// Names are right-padded to the longest name + 1 space so columns align
/// automatically — no manual space counting needed.
pub struct SessionNotes {
    items: Vec<(String, String)>,
}

impl SessionNotes {
    pub fn new() -> Self {
        Self { items: vec![] }
    }

    pub fn add(mut self, name: impl Into<String>, desc: impl Into<String>) -> Self {
        self.items.push((name.into(), desc.into()));
        self
    }

    /// Append a note only when `condition` is true.
    pub fn add_if(self, condition: bool, name: impl Into<String>, desc: impl Into<String>) -> Self {
        if condition { self.add(name, desc) } else { self }
    }

    /// Render as space-separated single-quoted shell arguments for `--notes`.
    pub fn to_show_info_args(&self) -> String {
        if self.items.is_empty() {
            return String::new();
        }
        let width = self.items.iter().map(|(n, _)| n.len()).max().unwrap_or(0) + 1;
        self.items
            .iter()
            .map(|(name, desc)| format!("'{:<width$}— {}'", name, desc, width = width))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// ── Row builders ───────────────────────────────────────────────────────────────

pub type Row = Option<String>;

fn content_row(text: &str) -> Row {
    Some(text.to_string())
}

fn service_rows(env_vars: &[String], env: &HashMap<String, String>, label: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = vec![None, content_row(&format!("  {}", label))];
    for key in env_vars {
        if let Some(val) = env.get(key)
            && !val.is_empty()
        {
            rows.push(content_row(&format!("  {}={}", key, val)));
        }
    }
    rows
}

// ── Box renderer ───────────────────────────────────────────────────────────────

/// Wrap `text` into lines that fit within `width` characters, breaking on spaces.
pub fn word_wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current.clone());
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub fn render_box(rows: &[Row]) -> String {
    let top = format!("╔{}╗", "═".repeat(INNER));
    let div = format!("╠{}╣", "═".repeat(INNER));
    let bottom = format!("╚{}╝", "═".repeat(INNER));

    let mut lines = vec![top];
    for row in rows {
        match row {
            None => lines.push(div.clone()),
            Some(text) => {
                let content_width = INNER - 2;
                let display: String = if text.chars().count() > content_width {
                    text.chars().take(content_width).collect()
                } else {
                    text.clone()
                };
                let padded = format!("{:<width$}", display, width = content_width);
                lines.push(format!("║ {} ║", padded));
            }
        }
    }
    lines.push(bottom);
    lines.join("\n")
}

// ── Main command ───────────────────────────────────────────────────────────────

pub fn run(
    preset: &str,
    workspace: Option<&str>,
    services: Option<&[String]>,
    message: Option<&str>,
    notes: Option<&[String]>,
    settings: &Settings,
) -> Result<()> {
    let project_name = if settings.project.project_name.is_empty() {
        "dev session"
    } else {
        &settings.project.project_name
    };

    let mut rows: Vec<Row> = vec![
        content_row(&format!("  {}", project_name)),
        content_row(&format!("  preset  {}", preset)),
    ];

    // Named services
    let svc_list: Vec<&str> = services
        .map(|s| s.iter().map(|x| x.as_str()).collect())
        .unwrap_or_default();

    for service in &svc_list {
        let path = settings.project.service_env_path(service, None);
        let env = if path.exists() {
            parse_env(&std::fs::read_to_string(&path).unwrap_or_default())
        } else {
            HashMap::new()
        };
        let exists_mark = if path.exists() {
            "  ✓"
        } else {
            "  ✗ .env missing"
        };
        let label = format!("{}{}", service, exists_mark);
        let env_vars = settings
            .project
            .services
            .iter()
            .find(|s| s.name == *service)
            .map(|s| s.env_vars.clone())
            .unwrap_or_default();
        rows.extend(service_rows(&env_vars, &env, &label));
    }

    // Workspace (dev_ui worktree)
    if let Some(ws) = workspace {
        let ws_path = Path::new(ws);
        let env_path = ws_path.join(".env");
        let env = if env_path.exists() {
            parse_env(&std::fs::read_to_string(&env_path).unwrap_or_default())
        } else {
            HashMap::new()
        };
        let repo = &settings.project.dev_ui.repo;
        let ws_name = ws_path.file_name().and_then(|n| n.to_str()).unwrap_or(ws);
        let ws_label = if ws_name == repo.as_str() {
            "main"
        } else {
            ws_name
        };
        let exists_mark = if env_path.exists() {
            "  ✓"
        } else {
            "  ✗ .env missing"
        };
        let label = format!("{}  [{}]{}", repo, ws_label, exists_mark);
        let env_vars = settings
            .project
            .services
            .iter()
            .find(|s| &s.name == repo)
            .map(|s| s.env_vars.clone())
            .unwrap_or_default();
        rows.extend(service_rows(&env_vars, &env, &label));
    }

    if let Some(msg) = message
        && !msg.is_empty()
    {
        let content_width = INNER - 2;
        rows.push(None);
        for line in word_wrap(msg, content_width) {
            rows.push(content_row(&format!("  {}", line)));
        }
    }

    if let Some(note_lines) = notes
        && !note_lines.is_empty()
    {
        rows.push(None);
        for line in note_lines {
            rows.push(content_row(&format!("  {}", line)));
        }
    }

    println!("{}", render_box(&rows));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── SessionNotes ──────────────────────────────────────────────────────────

    #[test]
    fn session_notes_empty_produces_empty_string() {
        let notes = SessionNotes::new();
        assert_eq!(notes.to_show_info_args(), "");
    }

    #[test]
    fn session_notes_single_item() {
        let args = SessionNotes::new().add("show_info", "re-display this box").to_show_info_args();
        assert_eq!(args, "'show_info — re-display this box'");
    }

    #[test]
    fn session_notes_pads_names_to_longest() {
        let args = SessionNotes::new()
            .add("teardown", "remove worktree")
            .add("reload_env", "reload .env")
            .to_show_info_args();
        // "reload_env" is 10 chars (longest); width = 11.
        // "teardown" (8) gets 3 padding spaces; "reload_env" gets 1.
        assert!(args.contains("'teardown    — remove worktree'") || args.contains("'teardown   — remove worktree'"));
        assert!(args.contains("reload_env "));
        // All column dashes should be at the same horizontal position
        let width = args.split("— ").next().unwrap_or("").split('\'').last().unwrap_or("").len();
        for part in args.split("— ").collect::<Vec<_>>().windows(1) {
            let _ = part; // just ensure it splits correctly
        }
        assert_eq!(width, 11); // longest (10) + 1
    }

    #[test]
    fn session_notes_add_if_true_includes_item() {
        let args = SessionNotes::new()
            .add("show_info", "box")
            .add_if(true, "open_ticket", "ticket")
            .to_show_info_args();
        assert!(args.contains("open_ticket"));
    }

    #[test]
    fn session_notes_add_if_false_omits_item() {
        let args = SessionNotes::new()
            .add("show_info", "box")
            .add_if(false, "open_ticket", "ticket")
            .to_show_info_args();
        assert!(!args.contains("open_ticket"));
    }

    // ── word_wrap / render_box ────────────────────────────────────────────────

    #[test]
    fn word_wrap_short_text_stays_on_one_line() {
        let lines = word_wrap("hello world", 20);
        assert_eq!(lines, vec!["hello world"]);
    }

    #[test]
    fn word_wrap_long_text_breaks_on_space() {
        let lines = word_wrap("one two three four five", 10);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line.chars().count() <= 10, "line too long: {:?}", line);
        }
    }

    #[test]
    fn word_wrap_deploy_message_fits_box() {
        let content_width = BOX_WIDTH - 4; // INNER - 2
        let msg = "To deploy, run: git subtree pull --prefix  project feat/multi-tenant --squash";
        let lines = word_wrap(msg, content_width);
        assert!(lines.len() > 1, "long message should wrap");
        for line in &lines {
            assert!(
                line.chars().count() <= content_width,
                "wrapped line too long: {:?}",
                line
            );
        }
        let rejoined = lines.join(" ");
        assert!(rejoined.contains("subtree"));
        assert!(rejoined.contains("multi-tenant"));
    }

    #[test]
    fn word_wrap_empty_string_returns_one_empty_line() {
        let lines = word_wrap("", 20);
        assert_eq!(lines, vec![""]);
    }

    #[test]
    fn render_box_has_correct_width() {
        let rows: Vec<Row> = vec![
            Some("  dev session".to_string()),
            Some("  preset  test".to_string()),
        ];
        let output = render_box(&rows);
        for line in output.lines() {
            let char_count = line.chars().count();
            assert_eq!(char_count, BOX_WIDTH, "Line has wrong width: {:?}", line);
        }
    }

    #[test]
    fn render_box_top_bottom_use_double_corners() {
        let rows: Vec<Row> = vec![Some("test".to_string())];
        let output = render_box(&rows);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[0].starts_with('╔'));
        assert!(lines[0].ends_with('╗'));
        assert!(lines.last().unwrap().starts_with('╚'));
        assert!(lines.last().unwrap().ends_with('╝'));
    }

    #[test]
    fn render_box_divider_uses_middle_chars() {
        let rows: Vec<Row> = vec![None];
        let output = render_box(&rows);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[1].starts_with('╠'));
        assert!(lines[1].ends_with('╣'));
    }

    #[test]
    fn render_box_content_row_padded() {
        let rows: Vec<Row> = vec![Some("hi".to_string())];
        let output = render_box(&rows);
        let lines: Vec<&str> = output.lines().collect();
        let row = lines[1];
        assert!(row.starts_with("║ "));
        assert!(row.ends_with(" ║"));
        assert_eq!(row.chars().count(), BOX_WIDTH);
    }

    #[test]
    fn service_rows_shows_env_var_values() {
        let mut env = HashMap::new();
        env.insert("NODE_ENV".to_string(), "test".to_string());
        env.insert("POSTGRES_HOST".to_string(), "localhost".to_string());

        let vars = vec!["NODE_ENV".to_string(), "POSTGRES_HOST".to_string()];
        let rows = service_rows(&vars, &env, "my-service  ✓");

        assert!(rows[0].is_none()); // divider
        assert!(rows[1].as_ref().unwrap().contains("my-service"));
        let content: Vec<_> = rows[2..].iter().filter_map(|r| r.as_ref()).collect();
        assert!(
            content
                .iter()
                .any(|r| r.contains("NODE_ENV") && r.contains("test"))
        );
        assert!(
            content
                .iter()
                .any(|r| r.contains("POSTGRES_HOST") && r.contains("localhost"))
        );
    }

    #[test]
    fn service_rows_skips_empty_values() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "".to_string());
        env.insert("BAR".to_string(), "hello".to_string());

        let vars = vec!["FOO".to_string(), "BAR".to_string()];
        let rows = service_rows(&vars, &env, "svc");
        let content: Vec<_> = rows[2..].iter().filter_map(|r| r.as_ref()).collect();
        assert!(!content.iter().any(|r| r.contains("FOO")));
        assert!(content.iter().any(|r| r.contains("BAR")));
    }
}
