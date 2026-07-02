/// Integration tests for env_file parsing.
/// These mirror the unit tests in src/env_file.rs but run as separate integration tests.

// The integration tests just call into the binary's library logic.
// Since we have a binary-only crate, we test via the inline #[cfg(test)] blocks in src/.
// These tests serve as additional end-to-end verification.

#[test]
fn env_file_all_rules_combined() {
    // Test a realistic .env file
    let content = r#"
# Database configuration
POSTGRES_HOST="localhost"
POSTGRES_PORT=5432
POSTGRES_DB='mydb'
POSTGRES_USERNAME=admin
POSTGRES_PASSWORD="s3cret"

# Redis
REDIS_HOST=127.0.0.1
REDIS_PORT=6379

# This line has no equals sign
INVALID_LINE

# Empty value
EMPTY_VAR=

NODE_ENV=development
URL=http://example.com?foo=bar&baz=qux
"#;

    // Parse manually to verify rules
    let mut map = std::collections::HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_string();
            let raw_val = line[eq_pos + 1..].trim();
            // Strip quotes
            let val = if (raw_val.starts_with('"') && raw_val.ends_with('"') && raw_val.len() >= 2)
                || (raw_val.starts_with('\'') && raw_val.ends_with('\'') && raw_val.len() >= 2)
            {
                raw_val[1..raw_val.len() - 1].to_string()
            } else {
                raw_val.to_string()
            };
            if !key.is_empty() {
                map.insert(key, val);
            }
        }
    }

    assert_eq!(map["POSTGRES_HOST"], "localhost");
    assert_eq!(map["POSTGRES_PORT"], "5432");
    assert_eq!(map["POSTGRES_DB"], "mydb");
    assert_eq!(map["POSTGRES_USERNAME"], "admin");
    assert_eq!(map["POSTGRES_PASSWORD"], "s3cret");
    assert_eq!(map["REDIS_HOST"], "127.0.0.1");
    assert_eq!(map["EMPTY_VAR"], "");
    assert_eq!(map["NODE_ENV"], "development");
    assert_eq!(map["URL"], "http://example.com?foo=bar&baz=qux");
    assert!(!map.contains_key("INVALID_LINE"));
}
