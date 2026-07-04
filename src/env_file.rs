/// .env file parsing and writing utilities.
use std::collections::HashMap;
use std::path::Path;

/// Convert days since Unix epoch (1970-01-01) to a Gregorian `(year, month, day)` triple.
///
/// Used by both `cmd::env_age` and `backend::sqlite` for timestamp formatting.
pub(crate) fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

/// Escape a string for safe embedding inside a single-quoted POSIX shell argument.
///
/// Replaces every `'` with `'\''` so the value can be wrapped in single quotes
/// without breaking the quoting or allowing injection.  E.g.:
///
/// ```
/// let cmd = format!("cd '{}'", penv::env_file::sh_escape("/home/alice's project"));
/// assert_eq!(cmd, "cd '/home/alice'\\''s project'");
/// ```
pub fn sh_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Parse the contents of a .env file into a key→value map.
///
/// Rules (matching the Python implementation):
/// - Skip blank lines
/// - Skip lines starting with `#`
/// - Skip lines without `=`
/// - Strip surrounding double or single quotes from values
pub fn parse_env(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_string();
            let raw_val = line[eq_pos + 1..].trim();
            let val = strip_quotes(raw_val).to_string();
            if !key.is_empty() {
                map.insert(key, val);
            }
        }
    }
    map
}

/// Strip a single layer of surrounding double or single quotes.
pub fn strip_quotes(s: &str) -> &str {
    for q in ['"', '\''] {
        let w = q.len_utf8();
        if s.len() >= 2 * w && s.starts_with(q) && s.ends_with(q) {
            return &s[w..s.len() - w];
        }
    }
    s
}

/// Read file contents, returning None if the file doesn't exist.
pub fn read_file(path: &Path) -> Option<String> {
    if path.exists() {
        std::fs::read_to_string(path).ok()
    } else {
        None
    }
}

/// Write content to a path, creating parent directories as needed.
pub fn write_file(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_kv() {
        let content = "FOO=bar\nBAZ=qux\n";
        let map = parse_env(content);
        assert_eq!(map["FOO"], "bar");
        assert_eq!(map["BAZ"], "qux");
    }

    #[test]
    fn skip_comments_and_blank_lines() {
        let content = "\n# this is a comment\nKEY=value\n\n# another comment\n";
        let map = parse_env(content);
        assert_eq!(map.len(), 1);
        assert_eq!(map["KEY"], "value");
    }

    #[test]
    fn skip_lines_without_equals() {
        let content = "INVALID_LINE\nVALID=yes\n";
        let map = parse_env(content);
        assert_eq!(map.len(), 1);
        assert_eq!(map["VALID"], "yes");
    }

    #[test]
    fn strip_double_quotes() {
        let map = parse_env("HOST=\"localhost\"\n");
        assert_eq!(map["HOST"], "localhost");
    }

    #[test]
    fn strip_single_quotes() {
        let map = parse_env("HOST='localhost'\n");
        assert_eq!(map["HOST"], "localhost");
    }

    #[test]
    fn no_strip_mismatched_quotes() {
        // mismatched quotes should be left as-is
        let map = parse_env("HOST='localhost\"\n");
        assert_eq!(map["HOST"], "'localhost\"");
    }

    #[test]
    fn value_with_equals_in_it() {
        // Only split on the first `=`
        let map = parse_env("URL=http://example.com?a=1\n");
        assert_eq!(map["URL"], "http://example.com?a=1");
    }

    #[test]
    fn empty_value() {
        let map = parse_env("EMPTY=\n");
        assert_eq!(map["EMPTY"], "");
    }

    #[test]
    fn strips_surrounding_quotes_not_inner() {
        let map = parse_env(r#"MSG="hello world""#);
        assert_eq!(map["MSG"], "hello world");
    }

    #[test]
    fn sh_escape_no_quotes() {
        assert_eq!(sh_escape("/home/user/project"), "/home/user/project");
    }

    #[test]
    fn sh_escape_single_quote() {
        assert_eq!(sh_escape("alice's"), "alice'\\''s");
    }

    #[test]
    fn sh_escape_multiple_quotes() {
        assert_eq!(sh_escape("it's a 'test'"), "it'\\''s a '\\''test'\\''");
    }
}
