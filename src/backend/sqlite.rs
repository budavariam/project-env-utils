//! SQLite secret backend — stores all (project, service, preset) env content in a local DB.
//!
//! Database location: `<repo_root>/local/secrets.db` (gitignored).
//! Table: env_secrets(project, service, preset, content, updated_at)
use std::path::PathBuf;

use rusqlite::{params, Connection};

use super::SecretBackend;
use crate::config::repo_root;

pub struct SqliteBackend {
    pub db_path: PathBuf,
    pub project_name: String,
}

impl SqliteBackend {
    pub fn new(db_path: PathBuf, project_name: &str) -> Self {
        Self {
            db_path,
            project_name: project_name.to_string(),
        }
    }

    pub fn default_path() -> PathBuf {
        repo_root().join("local").join("secrets.db")
    }

    fn connect(&self) -> Option<Connection> {
        if let Some(parent) = self.db_path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        let conn = Connection::open(&self.db_path).ok()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS env_secrets (
                project    TEXT NOT NULL,
                service    TEXT NOT NULL,
                preset     TEXT NOT NULL,
                content    TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (project, service, preset)
            );",
        )
        .ok()?;
        Some(conn)
    }
}

impl SecretBackend for SqliteBackend {
    fn label(&self) -> &'static str {
        "SQLite"
    }

    fn available(&self) -> bool {
        self.connect().is_some()
    }

    fn fetch(&self, service: &str, preset: &str) -> Option<String> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT content FROM env_secrets
             WHERE project = ?1 AND service = ?2 AND preset = ?3",
            params![self.project_name, service, preset],
            |row| row.get(0),
        )
        .ok()
    }

    fn exists(&self, service: &str, preset: &str) -> bool {
        self.fetch(service, preset).is_some()
    }

    fn push(&self, service: &str, preset: &str, content: &str) -> bool {
        let Some(conn) = self.connect() else {
            return false;
        };
        let now = iso_now();
        conn.execute(
            "INSERT OR REPLACE INTO env_secrets
             (project, service, preset, content, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![self.project_name, service, preset, content, now],
        )
        .is_ok()
    }

    fn delete(&self, service: &str, preset: &str) -> bool {
        let Some(conn) = self.connect() else {
            return false;
        };
        conn.execute(
            "DELETE FROM env_secrets
             WHERE project = ?1 AND service = ?2 AND preset = ?3",
            params![self.project_name, service, preset],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    fn list(&self) -> Vec<(String, String)> {
        let Some(conn) = self.connect() else {
            return vec![];
        };
        let mut stmt = match conn.prepare(
            "SELECT service, preset FROM env_secrets
             WHERE project = ?1
             ORDER BY service, preset",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let mut rows = match stmt.query(params![self.project_name]) {
            Ok(r) => r,
            Err(_) => return vec![],
        };
        let mut result = Vec::new();
        while let Ok(Some(row)) = rows.next() {
            if let (Ok(svc), Ok(preset)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                result.push((svc, preset));
            }
        }
        result
    }

    fn key_display(&self, service: &str, preset: &str) -> String {
        format!("{}::{}/{}", self.project_name, service, preset)
    }

    fn item_updated_at(&self, service: &str, preset: &str) -> Option<String> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT updated_at FROM env_secrets
             WHERE project = ?1 AND service = ?2 AND preset = ?3",
            params![self.project_name, service, preset],
            |row| row.get(0),
        )
        .ok()
    }
}

fn iso_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let tod = secs % 86400;
    let days = secs / 86400;
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;
    let (y, mo, d) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, hh, mm, ss)
}

fn days_to_ymd(days: i64) -> (i64, u32, u32) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::ENV_LOCK;

    #[test]
    fn sqlite_backend_push_fetch_delete() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("PENV_REPO_ROOT", dir.path().to_str().unwrap());

        let db_path = dir.path().join("test.db");
        let b = SqliteBackend::new(db_path, "myproject");

        assert!(b.available());
        assert!(!b.exists("svc-a", "test"));

        let ok = b.push("svc-a", "test", "FOO=bar\nBAZ=qux\n");
        assert!(ok, "push should succeed");
        assert!(b.exists("svc-a", "test"));

        let content = b.fetch("svc-a", "test").unwrap();
        assert_eq!(content, "FOO=bar\nBAZ=qux\n");

        let pairs = b.list();
        assert_eq!(pairs, vec![("svc-a".to_string(), "test".to_string())]);

        let deleted = b.delete("svc-a", "test");
        assert!(deleted);
        assert!(!b.exists("svc-a", "test"));

        std::env::remove_var("PENV_REPO_ROOT");
    }

    #[test]
    fn sqlite_backend_project_isolation() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("PENV_REPO_ROOT", dir.path().to_str().unwrap());

        let db_path = dir.path().join("test.db");
        let b1 = SqliteBackend::new(db_path.clone(), "project-a");
        let b2 = SqliteBackend::new(db_path, "project-b");

        b1.push("svc", "test", "PROJECT=a\n");
        b2.push("svc", "test", "PROJECT=b\n");

        assert_eq!(b1.fetch("svc", "test").unwrap(), "PROJECT=a\n");
        assert_eq!(b2.fetch("svc", "test").unwrap(), "PROJECT=b\n");
        assert_eq!(b1.list().len(), 1);
        assert_eq!(b2.list().len(), 1);

        std::env::remove_var("PENV_REPO_ROOT");
    }

    #[test]
    fn sqlite_backend_updated_at_set() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("PENV_REPO_ROOT", dir.path().to_str().unwrap());

        let db_path = dir.path().join("test.db");
        let b = SqliteBackend::new(db_path, "myproject");
        b.push("svc", "test", "X=1\n");
        let ts = b.item_updated_at("svc", "test").unwrap();
        assert!(ts.ends_with('Z'), "should be ISO UTC: {}", ts);
        assert!(ts.len() >= 20, "should be full ISO datetime: {}", ts);

        std::env::remove_var("PENV_REPO_ROOT");
    }
}
