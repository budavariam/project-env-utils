//! Integration tests for show_info box rendering and formatting.

const BOX_WIDTH: usize = 56;
const INNER: usize = BOX_WIDTH - 2;

fn render_box(rows: &[Option<String>]) -> String {
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

#[test]
fn box_all_lines_have_correct_width() {
    let rows: Vec<Option<String>> = vec![
        Some("  dev session".to_string()),
        Some("  preset  test".to_string()),
        None,
        Some("  my-service  ✓".to_string()),
        Some("  NODE_ENV=development".to_string()),
    ];
    let output = render_box(&rows);
    for line in output.lines() {
        let char_count = line.chars().count();
        assert_eq!(
            char_count, BOX_WIDTH,
            "Line has wrong character count {}: {:?}",
            char_count, line
        );
    }
}

#[test]
fn box_structure_top_middle_bottom() {
    let rows: Vec<Option<String>> =
        vec![Some("header".to_string()), None, Some("body".to_string())];
    let output = render_box(&rows);
    let lines: Vec<&str> = output.lines().collect();

    assert!(lines[0].starts_with('╔') && lines[0].ends_with('╗'));
    assert!(lines[1].starts_with("║ ") && lines[1].ends_with(" ║"));
    assert!(lines[2].starts_with('╠') && lines[2].ends_with('╣'));
    assert!(lines[3].starts_with("║ ") && lines[3].ends_with(" ║"));
    assert!(lines[4].starts_with('╚') && lines[4].ends_with('╝'));
}

#[test]
fn box_long_line_truncated_to_width() {
    let long_val = "X".repeat(60);
    let rows = vec![Some(long_val)];
    let output = render_box(&rows);
    for line in output.lines() {
        assert_eq!(line.chars().count(), BOX_WIDTH, "Line: {:?}", line);
    }
}

#[test]
fn box_with_env_var_content() {
    let rows: Vec<Option<String>> = vec![
        Some("  dev session".to_string()),
        Some("  preset  test".to_string()),
        None,
        Some("  my-service  ✓".to_string()),
        Some("  NODE_ENV=development".to_string()),
        Some("  POSTGRES_HOST=localhost".to_string()),
    ];
    let output = render_box(&rows);

    for (i, line) in output.lines().enumerate() {
        assert_eq!(
            line.chars().count(),
            BOX_WIDTH,
            "Line {} has wrong width: {:?}",
            i,
            line
        );
    }

    assert!(output.contains("dev session"));
    assert!(output.contains("preset  test"));
    assert!(output.contains("NODE_ENV"));
}
