//! Session multiplexer abstraction — tmux, byobu, and GNU Screen.
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::config::{SessionMultiplexer, Settings};

// ── Trait ─────────────────────────────────────────────────────────────────────

pub trait Mux: Send + Sync {
    fn name(&self) -> &'static str;

    fn session_exists(&self, name: &str) -> bool;
    fn sessions_with_prefix(&self, prefix: &str) -> Vec<String>;

    fn new_session(&self, name: &str, window: &str, dir: &str) -> Result<()>;
    fn new_window(&self, session: &str, window: &str, dir: &str) -> Result<()>;
    fn select_window(&self, session: &str, index: usize) -> Result<()>;
    fn send_keys(&self, target: &str, cmd: &str) -> Result<()>;
    fn run_output(&self, args: &[&str]) -> Result<String>;

    /// Replace the current process with an attach command. Never returns.
    fn attach(&self, session: &str) -> !;

    /// Return the pane/window ID of position 0 within a window.
    fn first_pane_id(&self, session: &str, window_idx: u32) -> Result<String>;

    /// Build a grid of pane IDs.  For tmux/byobu these are real pane IDs;
    /// for screen each cell is a new window number (as a string).
    fn build_pane_grid(
        &self,
        first_id: &str,
        n_rows: usize,
        n_cols: usize,
        dir: &str,
    ) -> Result<Vec<Vec<String>>>;

    fn list_panes(&self, session: &str) -> Result<Vec<(String, u32, u32)>>;

    /// Link an external session's window into `dst_session` as a new tab.
    /// Default is a no-op (screen does not support window linking).
    fn link_window_from(&self, src_session: &str, src_window: &str, dst_session: &str) -> Result<()> {
        let _ = (src_session, src_window, dst_session);
        Ok(())
    }
}

// ── Factory ───────────────────────────────────────────────────────────────────

pub fn active_mux(settings: &Settings) -> Box<dyn Mux> {
    match settings.session_mux {
        SessionMultiplexer::Tmux   => Box::new(TmuxLike::new("tmux")),
        SessionMultiplexer::Byobu  => Box::new(TmuxLike::new("byobu")),
        SessionMultiplexer::Screen => Box::new(ScreenMux),
    }
}

// ── TmuxLike (tmux + byobu) ───────────────────────────────────────────────────

struct TmuxLike {
    bin: &'static str,
}

impl TmuxLike {
    fn new(bin: &'static str) -> Self { Self { bin } }

    fn run(&self, args: &[&str]) -> Result<()> {
        let status = Command::new(self.bin)
            .args(args)
            .status()
            .with_context(|| format!("failed to spawn {} {:?}", self.bin, args))?;
        if !status.success() {
            bail!("{} {:?} exited with {}", self.bin, args, status);
        }
        Ok(())
    }
}

impl Mux for TmuxLike {
    fn name(&self) -> &'static str { self.bin }

    fn session_exists(&self, name: &str) -> bool {
        Command::new(self.bin)
            .args(["has-session", "-t", &format!("={}", name)])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn sessions_with_prefix(&self, prefix: &str) -> Vec<String> {
        let out = Command::new(self.bin)
            .args(["list-sessions", "-F", "#{session_name}"])
            .output();
        match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|s| s.starts_with(prefix))
                .map(|s| s.to_string())
                .collect(),
            _ => vec![],
        }
    }

    fn new_session(&self, name: &str, window: &str, dir: &str) -> Result<()> {
        self.run(&["new-session", "-d", "-s", name, "-n", window, "-c", dir])
    }

    fn new_window(&self, session: &str, window: &str, dir: &str) -> Result<()> {
        self.run(&["new-window", "-t", session, "-n", window, "-c", dir])
    }

    fn select_window(&self, session: &str, index: usize) -> Result<()> {
        self.run(&["select-window", "-t", &format!("{}:{}", session, index)])
    }

    fn send_keys(&self, target: &str, cmd: &str) -> Result<()> {
        self.run(&["send-keys", "-t", target, cmd, "Enter"])
    }

    fn run_output(&self, args: &[&str]) -> Result<String> {
        let out = Command::new(self.bin)
            .args(args)
            .output()
            .with_context(|| format!("failed to spawn {} {:?}", self.bin, args))?;
        if !out.status.success() {
            bail!(
                "{} {:?} exited with {}: {}",
                self.bin, args, out.status,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn attach(&self, session: &str) -> ! {
        let args: &[&str] = if self.bin == "byobu" {
            &["attach-session", "-t", session]
        } else {
            &["attach-session", "-t", &format!("={}", session)]
        };
        let err = Command::new(self.bin).args(args).exec();
        eprintln!("Failed to attach to {} session '{}': {}", self.bin, session, err);
        std::process::exit(1);
    }

    fn first_pane_id(&self, session: &str, window_idx: u32) -> Result<String> {
        let target = format!("{}:{}.0", session, window_idx);
        self.run_output(&["display-message", "-p", "-t", &target, "#{pane_id}"])
    }

    fn build_pane_grid(
        &self,
        first_pane: &str,
        n_rows: usize,
        n_cols: usize,
        dir: &str,
    ) -> Result<Vec<Vec<String>>> {
        let n_cols = n_cols.max(1).min(2);
        let n_rows = n_rows.max(1);
        let mut grid = vec![vec![String::new(); n_cols]; n_rows];
        grid[0][0] = first_pane.to_string();

        if n_cols == 2 {
            let p = self.run_output(&[
                "split-window", "-h", "-l", "50%", "-t", first_pane,
                "-c", dir, "-P", "-F", "#{pane_id}",
            ])?;
            if p.is_empty() { bail!("build_pane_grid: h-split returned empty pane id"); }
            grid[0][1] = p;
        }

        for row in 1..n_rows {
            let prev_left = grid[row - 1][0].clone();
            let p = self.run_output(&[
                "split-window", "-v", "-l", "50%", "-t", &prev_left,
                "-c", dir, "-P", "-F", "#{pane_id}",
            ])?;
            if p.is_empty() { bail!("build_pane_grid: v-split (col 0, row {}) empty", row); }
            grid[row][0] = p;

            if n_cols == 2 {
                let prev_right = grid[row - 1][1].clone();
                let p = self.run_output(&[
                    "split-window", "-v", "-l", "50%", "-t", &prev_right,
                    "-c", dir, "-P", "-F", "#{pane_id}",
                ])?;
                if p.is_empty() { bail!("build_pane_grid: v-split (col 1, row {}) empty", row); }
                grid[row][1] = p;
            }
        }
        Ok(grid)
    }

    fn list_panes(&self, session: &str) -> Result<Vec<(String, u32, u32)>> {
        let out = self.run_output(&[
            "list-panes", "-t", &format!("={}", session),
            "-F", "#{pane_id} #{pane_left} #{pane_top}",
        ])?;
        Ok(parse_panes(&out))
    }

    fn link_window_from(&self, src_session: &str, src_window: &str, dst_session: &str) -> Result<()> {
        self.run(&[
            "link-window",
            "-s", &format!("{}:{}", src_session, src_window),
            "-t", &format!("{}:", dst_session),
        ])
    }
}

// ── ScreenMux ─────────────────────────────────────────────────────────────────

/// GNU Screen backend.
///
/// Screen has no native pane IDs — each logical "pane" in the grid becomes
/// a separate screen window.  `build_pane_grid` returns window numbers
/// (as strings) instead of tmux pane IDs.
struct ScreenMux;

impl ScreenMux {
    fn run(&self, args: &[&str]) -> Result<()> {
        let status = Command::new("screen")
            .args(args)
            .status()
            .with_context(|| format!("failed to spawn screen {:?}", args))?;
        if !status.success() {
            bail!("screen {:?} exited with {}", args, status);
        }
        Ok(())
    }

    /// `screen -S session -p window -X stuff "cmd\n"`
    fn stuff(&self, session: &str, window: &str, cmd: &str) -> Result<()> {
        self.run(&["-S", session, "-p", window, "-X", "stuff", &format!("{}\n", cmd)])
    }

    /// Create a new named window inside an existing session and return its number.
    fn new_window_numbered(&self, session: &str, window_name: &str, dir: &str) -> Result<String> {
        // screen -S session -X screen -t window_name
        self.run(&["-S", session, "-X", "screen", "-t", window_name])?;
        if !dir.is_empty() {
            // Send a cd to the newly created window (it's the last window by default)
            self.stuff(session, window_name, &format!("cd '{}'", dir))?;
        }
        // Return the window title as the "pane ID"
        Ok(window_name.to_string())
    }
}

impl Mux for ScreenMux {
    fn name(&self) -> &'static str { "screen" }

    fn session_exists(&self, name: &str) -> bool {
        let out = Command::new("screen")
            .args(["-ls", name])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains(&format!(".{}", name)) || s.contains(&format!("\t{}", name))
            }
            Err(_) => false,
        }
    }

    fn sessions_with_prefix(&self, prefix: &str) -> Vec<String> {
        let out = Command::new("screen")
            .args(["-ls"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|line| {
                    // Lines look like: "\t12345.session_name\t(Detached)"
                    let trimmed = line.trim();
                    let name = trimmed.split_whitespace().next()?;
                    // Extract the part after the PID dot
                    let session = name.splitn(2, '.').nth(1).unwrap_or(name);
                    if session.starts_with(prefix) {
                        Some(session.to_string())
                    } else {
                        None
                    }
                })
                .collect(),
            Err(_) => vec![],
        }
    }

    fn new_session(&self, name: &str, window: &str, dir: &str) -> Result<()> {
        // -d -m: start detached; -S: name; -t: first window title
        self.run(&["-d", "-m", "-S", name, "-t", window])?;
        if !dir.is_empty() {
            self.stuff(name, window, &format!("cd '{}'", dir))?;
        }
        Ok(())
    }

    fn new_window(&self, session: &str, window: &str, dir: &str) -> Result<()> {
        self.new_window_numbered(session, window, dir)?;
        Ok(())
    }

    fn select_window(&self, session: &str, _index: usize) -> Result<()> {
        // screen -S session -X select 0 (select window 0)
        self.run(&["-S", session, "-X", "select", &_index.to_string()])
    }

    fn send_keys(&self, target: &str, cmd: &str) -> Result<()> {
        // target is "session:window_name" or just "window_name" if no colon
        let (session, window) = if let Some((s, w)) = target.split_once(':') {
            (s, w)
        } else {
            ("", target)
        };
        if session.is_empty() {
            // No session prefix — use screen's -X directly (assumes current session)
            self.run(&["-X", "select", window])?;
            self.run(&["-X", "stuff", &format!("{}\n", cmd)])
        } else {
            self.stuff(session, window, cmd)
        }
    }

    fn run_output(&self, _args: &[&str]) -> Result<String> {
        // Screen has no equivalent of tmux display-message; return empty
        Ok(String::new())
    }

    fn attach(&self, session: &str) -> ! {
        let err = Command::new("screen").args(["-r", session]).exec();
        eprintln!("Failed to attach to screen session '{}': {}", session, err);
        std::process::exit(1);
    }

    fn first_pane_id(&self, _session: &str, window_idx: u32) -> Result<String> {
        // For screen, "pane IDs" are just window indices as strings
        Ok(window_idx.to_string())
    }

    fn build_pane_grid(
        &self,
        _first_id: &str,
        n_rows: usize,
        n_cols: usize,
        _dir: &str,
    ) -> Result<Vec<Vec<String>>> {
        // For screen, just assign sequential window numbers.
        // The first window already exists (created by new_session / new_window).
        // Subsequent ones are created lazily when send_keys is called with their ID.
        let n_cols = n_cols.max(1).min(2);
        let n_rows = n_rows.max(1);
        let mut grid = vec![vec![String::new(); n_cols]; n_rows];
        let mut next = 0usize;
        for r in 0..n_rows {
            for c in 0..n_cols {
                grid[r][c] = next.to_string();
                next += 1;
            }
        }
        Ok(grid)
    }

    fn list_panes(&self, _session: &str) -> Result<Vec<(String, u32, u32)>> {
        // Screen has no panes; return a single synthetic entry
        Ok(vec![("0".to_string(), 0, 0)])
    }
}

// ── Legacy free functions (kept for compatibility; delegate to TmuxLike) ──────

/// Run a tmux command, returning an error if it fails.
pub fn tmux(args: &[&str]) -> Result<()> {
    TmuxLike::new("tmux").run(args)
}

/// Run a tmux command capturing stdout; returns trimmed output.
pub fn tmux_output(args: &[&str]) -> Result<String> {
    TmuxLike::new("tmux").run_output(args)
}

/// Send keys to a tmux target pane, appending Enter.
pub fn tmux_send_keys(target: &str, cmd: &str) -> Result<()> {
    TmuxLike::new("tmux").send_keys(target, cmd)
}

/// Return true if a tmux session with the given name exists.
pub fn tmux_session_exists(name: &str) -> bool {
    TmuxLike::new("tmux").session_exists(name)
}

/// Return all running tmux session names whose name starts with `prefix`.
pub fn sessions_with_prefix(prefix: &str) -> Vec<String> {
    TmuxLike::new("tmux").sessions_with_prefix(prefix)
}

/// Replace the current process with `tmux attach-session -t =<session>`.
pub fn exec_tmux_attach(session: &str) -> ! {
    TmuxLike::new("tmux").attach(session)
}

/// List panes in a tmux session.
pub fn list_panes(session: &str) -> Result<Vec<(String, u32, u32)>> {
    TmuxLike::new("tmux").list_panes(session)
}

/// Get the first pane ID of a window by index.
pub fn first_pane_id(session: &str, window_index: u32) -> Result<String> {
    TmuxLike::new("tmux").first_pane_id(session, window_index)
}

/// Build a 2-column grid of panes for one tmux window.
pub fn build_tab_panes(
    first_pane: &str,
    n_rows: usize,
    n_cols: usize,
    default_dir: &str,
) -> Result<Vec<Vec<String>>> {
    TmuxLike::new("tmux").build_pane_grid(first_pane, n_rows, n_cols, default_dir)
}

/// Link an external tmux session's window into a dev session.
pub fn setup_linked_window(
    session: &str,
    cfg: &crate::config::LinkedWindowConfig,
) -> Result<()> {
    if !cfg.enabled {
        return Ok(());
    }
    let mux = TmuxLike::new("tmux");
    let ext_session = if cfg.session_name.is_empty() { "claude" } else { cfg.session_name.as_str() };
    let window     = if cfg.window.is_empty() { ext_session } else { cfg.window.as_str() };
    let cmd        = if cfg.cmd.is_empty()    { ext_session } else { cfg.cmd.as_str() };
    let start_dir  = resolve_start_dir(&cfg.start_dir);

    if !mux.session_exists(ext_session) {
        mux.new_session(ext_session, window, &start_dir)?;
        mux.send_keys(&format!("{}:{}", ext_session, window), cmd)?;
    }
    mux.link_window_from(ext_session, window, session)
}

fn resolve_start_dir(raw: &str) -> String {
    if raw.is_empty() {
        ".".to_string()
    } else if raw.starts_with("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}{}", home, &raw[1..])
    } else {
        raw.to_string()
    }
}

// ── Pane helpers (kept for callers that use the raw parse function) ────────────

/// Parse `tmux list-panes -F "#{pane_id} #{pane_left} #{pane_top}"` output.
pub fn parse_panes(output: &str) -> Vec<(String, u32, u32)> {
    let mut panes: Vec<(String, u32, u32)> = output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let id   = parts[0].to_string();
                let left = parts[1].parse::<u32>().ok()?;
                let top  = parts[2].parse::<u32>().ok()?;
                Some((id, left, top))
            } else {
                None
            }
        })
        .collect();
    panes.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
    panes
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_panes_sorts_by_left_then_top() {
        let input = "%2 0 15\n%0 0 0\n%1 90 0\n";
        let panes = parse_panes(input);
        assert_eq!(panes[0].0, "%0");
        assert_eq!(panes[1].0, "%2");
        assert_eq!(panes[2].0, "%1");
    }

    #[test]
    fn parse_panes_empty_input() {
        assert!(parse_panes("").is_empty());
    }

    #[test]
    fn parse_panes_skips_malformed_lines() {
        let panes = parse_panes("%0 0 0\nbad line\n%1 10 5\n");
        assert_eq!(panes.len(), 2);
    }

    #[test]
    fn screen_build_pane_grid_2x2() {
        let mux = ScreenMux;
        let grid = mux.build_pane_grid("0", 2, 2, "/tmp").unwrap();
        assert_eq!(grid.len(), 2);
        assert_eq!(grid[0], vec!["0", "1"]);
        assert_eq!(grid[1], vec!["2", "3"]);
    }

    #[test]
    fn screen_build_pane_grid_1x1() {
        let mux = ScreenMux;
        let grid = mux.build_pane_grid("0", 1, 1, "/tmp").unwrap();
        assert_eq!(grid, vec![vec!["0"]]);
    }
}
