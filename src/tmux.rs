//! Session multiplexer abstraction — tmux, byobu, and GNU Screen.
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};

use crate::config::{SessionMultiplexer, Settings};
use crate::env_file::sh_escape;

// ── Trait ─────────────────────────────────────────────────────────────────────

pub trait Mux: Send + Sync {
    /// Human-readable backend name, used in log messages.
    #[allow(dead_code)]
    fn name(&self) -> &'static str;

    fn session_exists(&self, name: &str) -> bool;
    fn sessions_with_prefix(&self, prefix: &str) -> Vec<String>;

    fn new_session(&self, name: &str, window: &str, dir: &str) -> Result<()>;
    fn new_window(&self, session: &str, window: &str, dir: &str) -> Result<()>;
    fn select_window(&self, session: &str, index: usize) -> Result<()>;
    /// Focus a specific pane. No-op on backends that have no sub-window panes.
    fn select_pane(&self, _target: &str) -> Result<()> {
        Ok(())
    }
    fn send_keys(&self, target: &str, cmd: &str) -> Result<()>;

    /// Set a human-readable title on a pane (shown in pane border when pane-border-status is on).
    /// Sets both the tmux pane title (select-pane -T) and a user pane variable @pane_name.
    /// Using @pane_name in pane-border-format avoids shell preexec overriding the title:
    ///   set -g pane-border-format " #{?#{@pane_name},#{@pane_name},#T} "
    /// Default is a no-op so backends that don't support it compile without changes.
    fn set_pane_title(&self, _target: &str, _title: &str) -> Result<()> {
        Ok(())
    }

    /// Replace the current process with an attach command. Never returns.
    fn attach(&self, session: &str) -> !;

    /// Return the pane/window ID of position 0 within the given window index.
    fn first_pane_id(&self, session: &str, window_idx: u32) -> Result<String>;

    /// Split the target pane horizontally (left/right). Returns the new pane ID.
    fn split_h(&self, target: &str, dir: &str) -> Result<String>;
    /// Split the target pane vertically (top/bottom). Returns the new pane ID.
    fn split_v(&self, target: &str, dir: &str) -> Result<String>;

    /// Build a grid of pane IDs by calling split_h / split_v.
    /// grid[row][col] holds each pane's ID, with [0][0] = first_id (pre-existing).
    fn build_pane_grid(
        &self,
        first_id: &str,
        n_rows: usize,
        n_cols: usize,
        dir: &str,
    ) -> Result<Vec<Vec<String>>> {
        let n_cols = n_cols.clamp(1, 2);
        let n_rows = n_rows.max(1);
        let mut grid = vec![vec![String::new(); n_cols]; n_rows];
        grid[0][0] = first_id.to_string();
        if n_cols == 2 {
            grid[0][1] = self.split_h(first_id, dir)?;
        }
        for row in 1..n_rows {
            grid[row][0] = self.split_v(&grid[row - 1][0], dir)?;
            if n_cols == 2 {
                grid[row][1] = self.split_v(&grid[row - 1][1], dir)?;
            }
        }
        Ok(grid)
    }

    /// List panes in a session; used for diagnostics and tests.
    #[allow(dead_code)]
    fn list_panes(&self, session: &str) -> Result<Vec<(String, u32, u32)>>;

    /// Link an external session's window into `dst_session` as a new tab.
    /// Default is a no-op (screen does not support window linking).
    fn link_window_from(
        &self,
        src_session: &str,
        src_window: &str,
        dst_session: &str,
    ) -> Result<()> {
        let _ = (src_session, src_window, dst_session);
        Ok(())
    }
}

// ── Factory ───────────────────────────────────────────────────────────────────

pub fn active_mux(settings: &Settings) -> Box<dyn Mux> {
    match settings.session_mux {
        SessionMultiplexer::Tmux => Box::new(TmuxLike::new("tmux")),
        SessionMultiplexer::Byobu => Box::new(TmuxLike::new("byobu")),
        SessionMultiplexer::Screen => Box::new(ScreenMux),
    }
}

/// Create a linked window using the given multiplexer.
pub fn setup_linked_window(
    session: &str,
    cfg: &crate::config::LinkedWindowConfig,
    mux: &dyn Mux,
) -> Result<()> {
    if !cfg.enabled {
        return Ok(());
    }
    let ext_session = if cfg.session_name.is_empty() {
        "claude"
    } else {
        cfg.session_name.as_str()
    };
    let window = if cfg.window.is_empty() {
        ext_session
    } else {
        cfg.window.as_str()
    };
    let cmd = if cfg.cmd.is_empty() {
        ext_session
    } else {
        cfg.cmd.as_str()
    };
    let start_dir = resolve_start_dir(&cfg.start_dir);

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

// ── TmuxLike (tmux + byobu) ───────────────────────────────────────────────────

struct TmuxLike {
    bin: &'static str,
}

impl TmuxLike {
    fn new(bin: &'static str) -> Self {
        Self { bin }
    }

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

    fn run_output(&self, args: &[&str]) -> Result<String> {
        let out = Command::new(self.bin)
            .args(args)
            .output()
            .with_context(|| format!("failed to spawn {} {:?}", self.bin, args))?;
        if !out.status.success() {
            bail!(
                "{} {:?} exited with {}: {}",
                self.bin,
                args,
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

impl Mux for TmuxLike {
    fn name(&self) -> &'static str {
        self.bin
    }

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

    fn select_pane(&self, target: &str) -> Result<()> {
        self.run(&["select-pane", "-t", target])
    }

    fn send_keys(&self, target: &str, cmd: &str) -> Result<()> {
        self.run(&["send-keys", "-t", target, cmd, "Enter"])
    }

    fn set_pane_title(&self, target: &str, title: &str) -> Result<()> {
        // Set both the tmux pane title and a user variable immune to shell overrides.
        self.run(&["select-pane", "-T", title, "-t", target]).ok();
        self.run(&["set-option", "-p", "-t", target, "@pane_name", title])
    }

    fn attach(&self, session: &str) -> ! {
        let args: &[&str] = if self.bin == "byobu" {
            &["attach-session", "-t", session]
        } else {
            &["attach-session", "-t", &format!("={}", session)]
        };
        let err = Command::new(self.bin).args(args).exec();
        eprintln!(
            "Failed to attach to {} session '{}': {}",
            self.bin, session, err
        );
        std::process::exit(1);
    }

    fn first_pane_id(&self, session: &str, window_idx: u32) -> Result<String> {
        let target = format!("{}:{}.0", session, window_idx);
        self.run_output(&["display-message", "-p", "-t", &target, "#{pane_id}"])
    }

    fn split_h(&self, target: &str, dir: &str) -> Result<String> {
        let p = self.run_output(&[
            "split-window",
            "-h",
            "-l",
            "50%",
            "-t",
            target,
            "-c",
            dir,
            "-P",
            "-F",
            "#{pane_id}",
        ])?;
        if p.is_empty() {
            bail!("split_h: empty pane id returned for target {}", target);
        }
        Ok(p)
    }

    fn split_v(&self, target: &str, dir: &str) -> Result<String> {
        let p = self.run_output(&[
            "split-window",
            "-v",
            "-l",
            "50%",
            "-t",
            target,
            "-c",
            dir,
            "-P",
            "-F",
            "#{pane_id}",
        ])?;
        if p.is_empty() {
            bail!("split_v: empty pane id returned for target {}", target);
        }
        Ok(p)
    }

    fn list_panes(&self, session: &str) -> Result<Vec<(String, u32, u32)>> {
        let out = self.run_output(&[
            "list-panes",
            "-t",
            &format!("={}", session),
            "-F",
            "#{pane_id} #{pane_left} #{pane_top}",
        ])?;
        Ok(parse_panes(&out))
    }

    fn link_window_from(
        &self,
        src_session: &str,
        src_window: &str,
        dst_session: &str,
    ) -> Result<()> {
        self.run(&[
            "link-window",
            "-s",
            &format!("{}:{}", src_session, src_window),
            "-t",
            &format!("{}:", dst_session),
        ])
    }
}

// ── ScreenMux ─────────────────────────────────────────────────────────────────

/// Global counter for generating unique screen window titles.
/// Screen doesn't have pane IDs; each logical pane becomes a named window.
static SCREEN_WINDOW_COUNTER: AtomicUsize = AtomicUsize::new(1);

/// GNU Screen backend.
///
/// Screen has no native pane splitting — each logical pane in the grid becomes
/// a separate screen window.  Pane IDs use the format `"session:window_spec"`
/// where `window_spec` is either a window index (from `first_pane_id`) or an
/// auto-generated title (from `split_h` / `split_v`).
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

    fn stuff(&self, session: &str, window: &str, cmd: &str) -> Result<()> {
        self.run(&[
            "-S",
            session,
            "-p",
            window,
            "-X",
            "stuff",
            &format!("{}\n", cmd),
        ])
    }

    /// Extract the session name from a `"session:window"` pane ID.
    fn session_of(target: &str) -> &str {
        target.split_once(':').map(|(s, _)| s).unwrap_or(target)
    }

    /// Create a new screen window in `session` with a unique auto title.
    fn new_split(&self, session: &str, dir: &str) -> Result<String> {
        let n = SCREEN_WINDOW_COUNTER.fetch_add(1, Ordering::SeqCst);
        let title = format!("p{}", n);
        self.run(&["-S", session, "-X", "screen", "-t", &title])?;
        if !dir.is_empty() {
            self.stuff(session, &title, &format!("cd '{}'", sh_escape(dir)))?;
        }
        Ok(format!("{}:{}", session, title))
    }
}

impl Mux for ScreenMux {
    fn name(&self) -> &'static str {
        "screen"
    }

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
                    let name = line.split_whitespace().next()?;
                    let session = name.split_once('.').map(|(_, s)| s).unwrap_or(name);
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
        self.run(&["-d", "-m", "-S", name, "-t", window])?;
        if !dir.is_empty() {
            self.stuff(name, window, &format!("cd '{}'", sh_escape(dir)))?;
        }
        Ok(())
    }

    fn new_window(&self, session: &str, window: &str, dir: &str) -> Result<()> {
        self.run(&["-S", session, "-X", "screen", "-t", window])?;
        if !dir.is_empty() {
            self.stuff(session, window, &format!("cd '{}'", sh_escape(dir)))?;
        }
        Ok(())
    }

    fn select_window(&self, session: &str, index: usize) -> Result<()> {
        self.run(&["-S", session, "-X", "select", &index.to_string()])
    }

    fn send_keys(&self, target: &str, cmd: &str) -> Result<()> {
        let (session, window) = target.split_once(':').unwrap_or(("", target));
        if session.is_empty() {
            self.run(&["-X", "select", window])?;
            self.run(&["-X", "stuff", &format!("{}\n", cmd)])
        } else {
            self.stuff(session, window, cmd)
        }
    }

    fn attach(&self, session: &str) -> ! {
        let err = Command::new("screen").args(["-r", session]).exec();
        eprintln!("Failed to attach to screen session '{}': {}", session, err);
        std::process::exit(1);
    }

    /// Returns `"session:N"` where N is the window index.
    /// Screen accepts both window indices and titles in its `-p` flag.
    fn first_pane_id(&self, session: &str, window_idx: u32) -> Result<String> {
        Ok(format!("{}:{}", session, window_idx))
    }

    /// Creates a new screen window for the split; screen has no visual h/v distinction.
    fn split_h(&self, target: &str, dir: &str) -> Result<String> {
        self.new_split(Self::session_of(target), dir)
    }

    fn split_v(&self, target: &str, dir: &str) -> Result<String> {
        self.new_split(Self::session_of(target), dir)
    }

    fn list_panes(&self, _session: &str) -> Result<Vec<(String, u32, u32)>> {
        Ok(vec![("0".to_string(), 0, 0)])
    }
}

// ── Parse helpers ──────────────────────────────────────────────────────────────

/// Parse `tmux list-panes -F "#{pane_id} #{pane_left} #{pane_top}"` output.
// Used by TmuxLike::list_panes and by unit tests; not called from the binary entry point.
#[allow(dead_code)]
pub(crate) fn parse_panes(output: &str) -> Vec<(String, u32, u32)> {
    let mut panes: Vec<(String, u32, u32)> = output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let id = parts[0].to_string();
                let left = parts[1].parse::<u32>().ok()?;
                let top = parts[2].parse::<u32>().ok()?;
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
    fn screen_first_pane_id_format() {
        let mux = ScreenMux;
        let id = mux.first_pane_id("mysession", 0).unwrap();
        assert_eq!(id, "mysession:0");
        let id2 = mux.first_pane_id("mysession", 1).unwrap();
        assert_eq!(id2, "mysession:1");
    }

    #[test]
    fn screen_session_of_extracts_session() {
        assert_eq!(ScreenMux::session_of("mysession:p1"), "mysession");
        assert_eq!(ScreenMux::session_of("mysession:0"), "mysession");
        // no colon → returns whole string as session (fallback)
        assert_eq!(ScreenMux::session_of("mysession"), "mysession");
    }
}
