//! `penv env-age [--service <name>] [--preset <name>]`
//!
//! Report modification timestamps for env files across three locations —
//! service repo .env, -utils local cache, and the secret backend — then
//! offer interactive actions to propagate or sync between them.
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;

use crate::backend::SecretBackend;
use crate::config::{repo_root, Settings};
use crate::env_file::read_file;

// ── Time helpers ───────────────────────────────────────────────────────────────

fn file_secs(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_utc(secs: u64) -> String {
    let s = secs as i64;
    let tod = s % 86400;
    let days = s / 86400;
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;
    let (y, mo, d) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC", y, mo, d, hh, mm, ss)
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

fn age_str(secs: u64) -> String {
    if secs < 120 {
        format!("{}s", secs)
    } else if secs < 7200 {
        format!("{}m", secs / 60)
    } else if secs < 172800 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    }
}

/// Parse ISO 8601 UTC ("2024-01-15T14:23:45Z") to unix seconds.
pub fn parse_iso_secs(ts: &str) -> Option<u64> {
    let s = ts.trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;
    let d: Vec<u64> = date.split('-').filter_map(|x| x.parse().ok()).collect();
    let t: Vec<u64> = time
        .split(':')
        .filter_map(|x| x.split('.').next()?.parse::<u64>().ok())
        .collect();
    if d.len() < 3 || t.len() < 3 {
        return None;
    }
    let y = d[0] as i64;
    let m = d[1] as u32;
    let day = d[2] as u32;
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2 / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * m2 as i64 + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days as u64 * 86400 + t[0] * 3600 + t[1] * 60 + t[2])
}

// ── Location record ────────────────────────────────────────────────────────────

struct Loc {
    name: String,
    path_display: String,
    secs: Option<u64>,
    content: Option<String>,
    note: Option<&'static str>,
}

impl Loc {
    fn ts_str(&self) -> String {
        self.secs.map(format_utc).unwrap_or_else(|| "—".to_string())
    }
}

// ── Input helper ───────────────────────────────────────────────────────────────

fn read_line() -> Option<String> {
    io::stdin().lock().lines().next().and_then(|r| r.ok())
}

// ── Main entry ────────────────────────────────────────────────────────────────

pub fn run(
    service: Option<&str>,
    preset: &str,
    backend: Option<&dyn SecretBackend>,
    settings: &Settings,
) -> Result<()> {
    let services: Vec<String> = match service {
        Some(s) => vec![s.to_string()],
        None => settings.project.services.iter().map(|s| s.name.clone()).collect(),
    };
    for svc in &services {
        println!();
        run_one(svc, preset, backend, settings)?;
    }
    Ok(())
}

// ── Per-service logic ─────────────────────────────────────────────────────────

fn run_one(service: &str, preset: &str, backend: Option<&dyn SecretBackend>, settings: &Settings) -> Result<()> {
    let svc_path = settings.project.service_env_path(service, None);
    let cache_path = settings.project.preset_local_path(service, preset);

    // Path label for the cache relative to penv root.
    let root = repo_root();
    let cache_display = cache_path
        .strip_prefix(&root)
        .map(|p| format!("<penv-root>/{}", p.to_string_lossy()))
        .unwrap_or_else(|_| cache_path.to_string_lossy().into_owned());

    let mut locs: Vec<Loc> = vec![
        Loc {
            name: "service .env".to_string(),
            path_display: svc_path.to_string_lossy().into_owned(),
            secs: file_secs(&svc_path),
            content: read_file(&svc_path),
            note: None,
        },
        Loc {
            name: "local cache".to_string(),
            path_display: cache_display,
            secs: file_secs(&cache_path),
            content: read_file(&cache_path),
            note: None,
        },
    ];

    if let Some(b) = backend {
        let ts_raw = b.item_updated_at(service, preset);
        let secs = ts_raw.as_deref().and_then(parse_iso_secs);
        let content = b.fetch(service, preset);
        locs.push(Loc {
            name: b.label().to_string(),
            path_display: b.key_display(service, preset),
            secs,
            content,
            note: Some("item-level timestamp"),
        });
    }

    // ── Display ────────────────────────────────────────────────────────────────

    let now = now_secs();
    let newest_secs = locs.iter().filter_map(|l| l.secs).max();
    let divider = "─".repeat(62);

    println!("  ╾─ env-age: {} [{}]", service, preset);
    println!("  {}", divider);
    println!("  {:<16}  {:<23}  {}", "Location", "Modified (UTC)", "Age");
    println!("  {}", divider);

    for (i, loc) in locs.iter().enumerate() {
        let ts = loc.ts_str();
        let age = match loc.secs {
            Some(s) => {
                let ago = if now >= s { age_str(now - s) } else { "0s".to_string() };
                if Some(s) == newest_secs {
                    format!("★ {} ago", ago)
                } else {
                    format!("{} ago", ago)
                }
            }
            None => "not found".to_string(),
        };
        let note_str = loc
            .note
            .map(|n| format!("  ({})", n))
            .unwrap_or_default();
        println!("  {:<16}  {:<23}  {}{}", loc.name, ts, age, note_str);
        println!("                     {}", loc.path_display);
        if i < locs.len() - 1 {
            println!();
        }
    }

    // ── Comparisons ────────────────────────────────────────────────────────────

    println!("\n  {}", divider);
    let n = locs.len();
    for i in 0..n {
        for j in (i + 1)..n {
            let a = &locs[i];
            let b_loc = &locs[j];
            match (a.secs, b_loc.secs) {
                (Some(sa), Some(sb)) => {
                    if sa == sb {
                        println!("  {} ↔ {}  identical timestamp", a.name, b_loc.name);
                    } else {
                        let (newer, older, diff) = if sa > sb {
                            (&a.name, &b_loc.name, sa - sb)
                        } else {
                            (&b_loc.name, &a.name, sb - sa)
                        };
                        println!(
                            "  {:<16} is {} NEWER than  {}",
                            newer,
                            age_str(diff),
                            older
                        );
                    }
                }
                _ => {
                    let missing = if a.secs.is_none() { &a.name } else { &b_loc.name };
                    println!("  {} — not available, skipping comparison", missing);
                }
            }
        }
    }

    // ── Actions ────────────────────────────────────────────────────────────────

    println!("\n  ── Actions ─────────────────────────────────────────────────────────");

    let mut menu: Vec<(String, String, usize, usize)> = Vec::new();

    // [a] propagate newest to all other locations (if there's a clear source)
    let propagate_src = newest_secs.and_then(|ns| {
        locs.iter()
            .position(|l| l.secs == Some(ns) && l.content.is_some())
    });
    if let Some(si) = propagate_src {
        let targets: Vec<&str> = locs
            .iter()
            .enumerate()
            .filter(|(i, l)| *i != si && (l.content.is_none() || l.secs != newest_secs))
            .map(|(_, l)| l.name.as_str())
            .collect();
        if !targets.is_empty() {
            println!(
                "    [a] propagate newest everywhere  ({} → {})",
                locs[si].name,
                targets.join(" + ")
            );
        }
    }

    // Individual directional options
    let mut key_n = 1usize;
    for from in 0..n {
        for to in 0..n {
            if from != to && locs[from].content.is_some() {
                let key = key_n.to_string();
                println!(
                    "    [{}] {:<16} → {}",
                    key, locs[from].name, locs[to].name
                );
                menu.push((key, String::new(), from, to));
                key_n += 1;
            }
        }
    }
    println!("    [s] skip");

    // Read choice
    print!("\n  Choice [s]: ");
    io::stdout().flush().ok();
    let raw = read_line().unwrap_or_default();
    let choice = raw.trim().to_lowercase();
    let choice = if choice.is_empty() { "s".to_string() } else { choice };

    if choice == "s" {
        println!("  Skipped.");
        return Ok(());
    }

    if choice == "a" {
        if let Some(si) = propagate_src {
            let content = locs[si].content.clone().unwrap();
            let src = locs[si].name.clone();
            for ti in 0..n {
                if ti == si {
                    continue;
                }
                apply_sync(service, preset, &locs, backend, &content, &src, ti, &svc_path, &cache_path)?;
            }
        }
        return Ok(());
    }

    if let Some((_, _, fi, ti)) = menu.iter().find(|(k, _, _, _)| *k == choice) {
        let content = locs[*fi].content.clone().unwrap();
        let src = locs[*fi].name.clone();
        apply_sync(service, preset, &locs, backend, &content, &src, *ti, &svc_path, &cache_path)?;
    } else {
        println!("  Unknown choice '{}'. Skipped.", choice);
    }

    Ok(())
}

// ── Apply a single sync action ─────────────────────────────────────────────────

fn apply_sync(
    service: &str,
    preset: &str,
    locs: &[Loc],
    backend: Option<&dyn SecretBackend>,
    content: &str,
    source_name: &str,
    target_idx: usize,
    svc_path: &Path,
    cache_path: &Path,
) -> Result<()> {
    let target_name = &locs[target_idx].name;
    match target_idx {
        0 => {
            // service .env
            crate::backup::backup_env_before_write(
                service,
                svc_path,
                "env-age-sync",
                &format!("{} → service .env", source_name),
            );
            crate::env_file::write_file(svc_path, content)?;
            println!("  Written → service .env  ({})", svc_path.display());
        }
        1 => {
            // local cache
            if let Some(parent) = cache_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            crate::backup::backup_env_before_write(
                service,
                cache_path,
                "env-age-sync",
                &format!("{} → local cache", source_name),
            );
            crate::env_file::write_file(cache_path, content)?;
            println!("  Written → local cache  ({})", cache_path.display());
        }
        _ => {
            // backend
            if let Some(b) = backend {
                crate::backup::log_backend_push(
                    service,
                    "env-age-sync",
                    &format!("{} → {}", source_name, b.label()),
                );
                if b.push(service, preset, content) {
                    b.post_push(service, preset);
                    println!("  Pushed → {}  ({})", b.label(), target_name);
                } else {
                    println!("  Push to {} failed.", b.label());
                }
            } else {
                println!("  No backend configured — cannot push.");
            }
        }
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_iso_secs_basic() {
        // 1970-01-01T00:00:00Z = 0
        assert_eq!(parse_iso_secs("1970-01-01T00:00:00Z"), Some(0));
        // 1970-01-01T00:01:00Z = 60
        assert_eq!(parse_iso_secs("1970-01-01T00:01:00Z"), Some(60));
    }

    #[test]
    fn parse_iso_secs_known_date() {
        // 2024-01-15T14:23:45Z
        // Days from epoch to 2024-01-15:
        // 2024 is a leap year.  Jan 15 = day 15, zero-indexed from Jan 1 = 14 days.
        // Use the round-trip: format and re-parse.
        let secs = parse_iso_secs("2024-01-15T14:23:45Z").unwrap();
        assert!(secs > 1_700_000_000, "should be a plausible 2024 timestamp");
        assert!(secs < 1_800_000_000, "should be well before 2026");
        // Round-trip: format then verify it looks like UTC
        let formatted = format_utc(secs);
        assert!(formatted.starts_with("2024-01-15 14:23:45"), "formatted: {}", formatted);
    }

    #[test]
    fn age_str_formatting() {
        assert_eq!(age_str(30), "30s");
        assert_eq!(age_str(90), "90s");
        assert_eq!(age_str(120), "2m");
        assert_eq!(age_str(3600), "60m");
        assert_eq!(age_str(7200), "2h 0m");
        assert_eq!(age_str(9000), "2h 30m");
        assert_eq!(age_str(86400), "24h 0m");
        assert_eq!(age_str(172800), "2d 0h");
        assert_eq!(age_str(176400), "2d 1h");
    }

    #[test]
    fn format_utc_epoch() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00 UTC");
    }

    #[test]
    fn parse_iso_secs_with_subseconds() {
        // Some backends emit "2024-01-15T14:23:45.000Z"
        let a = parse_iso_secs("2024-01-15T14:23:45Z");
        let b = parse_iso_secs("2024-01-15T14:23:45.000Z");
        assert_eq!(a, b);
    }
}
