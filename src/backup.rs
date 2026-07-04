/// Lightweight pre-write backup and append-mode reflog for all env mutations.
///
/// Every code path that overwrites a service .env file calls
/// `backup_env_before_write` first.  This does two things:
///
///   1. Snapshot — saves the current file content to
///      `backups/<timestamp>-<cmd>/<label>.before.env` so the previous state
///      is always recoverable.
///
///   2. Reflog — appends one tab-separated line to `backups/reflog.txt`
///      (opened in append mode, so every entry survives a crash mid-run):
///
///      <timestamp>  <command>  <label>  <file>  <reason>  <snapshot|"(new)">
///
/// The reflog serves the same role as `git reflog`: a permanent, append-only
/// audit trail of what changed, when, by which command, and why — with a
/// pointer to the snapshot so you can recover the exact previous state.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::config::backups_dir;

// ── Public API ─────────────────────────────────────────────────────────────────

/// Back up `dest` before it is overwritten and write a reflog entry.
///
/// * `label`   — human-readable identifier, e.g. `"-ui/test"`
/// * `dest`    — path about to be overwritten
/// * `command` — the penv sub-command responsible, e.g. `"load-env"`
/// * `reason`  — one-line explanation of why the write is happening
///
/// Returns the snapshot path if the file existed before the call, or `None`
/// if it is being created fresh (still writes a reflog entry either way).
///
/// This function is infallible: backup failures are silently ignored so they
/// never block the main operation.
pub fn backup_env_before_write(
    label: &str,
    dest: &Path,
    command: &str,
    reason: &str,
) -> Option<PathBuf> {
    let ts = timestamp_now();
    let existing = std::fs::read_to_string(dest).ok();

    let snapshot_path = if let Some(ref content) = existing {
        // One directory per backup event: backups/<ts>-<cmd>/
        let safe_cmd = command.replace(|c: char| !c.is_alphanumeric() && c != '-', "-");
        let dir = backups_dir().join(format!("{}-{}", ts, safe_cmd));
        let _ = std::fs::create_dir_all(&dir);
        let safe_label = label.replace('/', ".").replace(char::is_whitespace, "_");
        let snap = dir.join(format!("{}.before.env", safe_label));
        let _ = std::fs::write(&snap, content);
        Some(snap)
    } else {
        None
    };

    let snapshot_note = match &snapshot_path {
        Some(p) => p.display().to_string(),
        None => "(new file — no previous content)".to_string(),
    };
    let entry = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\n",
        ts,
        command,
        label,
        dest.display(),
        reason,
        snapshot_note,
    );
    append_reflog(&entry);

    snapshot_path
}

/// Log a backend push to the reflog without saving a local snapshot
/// (the local file is not being modified; the backend state is changing).
pub fn log_backend_push(label: &str, command: &str, reason: &str) {
    let entry = format!(
        "{}\t{}\t{}\t(backend)\t{}\t(backend overwrite — no local snapshot)\n",
        timestamp_now(),
        command,
        label,
        reason,
    );
    append_reflog(&entry);
}

// ── Internals ──────────────────────────────────────────────────────────────────

fn append_reflog(entry: &str) {
    let path = backups_dir().join("reflog.txt");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
    {
        let _ = f.write_all(entry.as_bytes());
    }
}

fn timestamp_now() -> String {
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let (y, mo, d, h, mi, s) = unix_to_ymdhms(dur.as_secs());
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}Z", y, mo, d, h, mi, s)
}

fn unix_to_ymdhms(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let sec = secs % 60;
    let min = (secs / 60) % 60;
    let hour = (secs / 3600) % 24;
    let mut days = secs / 86400;
    let mut year = 1970u64;
    loop {
        let in_year = if is_leap(year) { 366 } else { 365 };
        if days < in_year {
            break;
        }
        days -= in_year;
        year += 1;
    }
    let month_days: [u64; 12] = [
        31,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u64;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }
    (year, month, days + 1, hour, min, sec)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}
