# Extending penv

`project-env-utils` is both a standalone binary (`penv`) and a Rust library. You can wrap it to add project-specific secret backends, custom subcommands, or shell scripts — without forking or duplicating the shared logic.

---

## When to extend vs use penv directly

Use `penv` as-is when:
- 1Password is your only secret backend
- You have no project-specific commands beyond preset management

Create a wrapper when:
- You need an additional secret backend (e.g. a custom vault, SQLite, an internal secrets service)
- You want project-specific subcommands baked into the binary
- You want the binary to have your project's name (e.g. `myproject-env`)
- You want to ship project-specific scripts that call the binary

---

## Architecture overview

```
project-env-utils/
  src/lib.rs     ← public API: all backends, commands, config types
  src/main.rs    ← thin CLI wrapper using the lib

my-project-env-utils/   ← your wrapper repo
  src/
    main.rs              ← extended CLI: penv commands + your own
    backend/
      my_vault.rs        ← your custom backend
    cmd/
      my_sync.rs         ← your custom subcommands
  Cargo.toml             ← depends on penv as a path/git dependency
```

`Cargo.toml` dependency:

```toml
[dependencies]
penv = { path = "../project-env-utils", package = "project-env-utils" }
clap = { version = "4", features = ["derive"] }
anyhow = "1"
```

Once your project stabilises, you can pin to a git commit instead of a path:

```toml
penv = { git = "https://github.com/your-org/project-env-utils", rev = "abc1234", package = "project-env-utils" }
```

---

## Implementing a custom backend

Any backend must implement the `penv::backend::SecretBackend` trait:

```rust
pub trait SecretBackend: Send + Sync {
    fn available(&self) -> bool;
    fn fetch(&self, service: &str, preset: &str) -> Option<String>;
    fn exists(&self, service: &str, preset: &str) -> bool;
    fn push(&self, service: &str, preset: &str, content: &str) -> bool;
    fn delete(&self, service: &str, preset: &str) -> bool;
    fn list(&self) -> Vec<(String, String)>;
    fn label(&self) -> &'static str;
    // optional:
    fn key_display(&self, service: &str, preset: &str) -> String { ... }
    fn post_push(&self, service: &str, preset: &str) {}
}
```

### Example: SQLite backend

A backend that stores env files in a local SQLite database — useful for offline use or when you don't want a cloud backend.

```rust
// src/backend/sqlite_vault.rs
use penv::backend::SecretBackend;

pub struct SqliteVault {
    db_path: std::path::PathBuf,
}

impl SqliteVault {
    pub fn new(db_path: impl Into<std::path::PathBuf>) -> Self {
        SqliteVault { db_path: db_path.into() }
    }
}

impl SecretBackend for SqliteVault {
    fn available(&self) -> bool {
        // Check the DB file is accessible (or create it)
        self.db_path.parent().map(|p| p.exists()).unwrap_or(false)
    }

    fn fetch(&self, service: &str, preset: &str) -> Option<String> {
        // SELECT content FROM presets WHERE service = ? AND preset = ?
        todo!()
    }

    fn push(&self, service: &str, preset: &str, content: &str) -> bool {
        // INSERT OR REPLACE INTO presets (service, preset, content) VALUES (?, ?, ?)
        todo!()
    }

    fn exists(&self, service: &str, preset: &str) -> bool {
        self.fetch(service, preset).is_some()
    }

    fn delete(&self, service: &str, preset: &str) -> bool {
        // DELETE FROM presets WHERE service = ? AND preset = ?
        todo!()
    }

    fn list(&self) -> Vec<(String, String)> {
        // SELECT service, preset FROM presets
        todo!()
    }

    fn label(&self) -> &'static str { "SQLite" }
}
```

### Example: Internal HTTP secrets service

```rust
pub struct HttpVault {
    base_url: String,
    token: String,
}

impl SecretBackend for HttpVault {
    fn available(&self) -> bool {
        // GET /health
        std::process::Command::new("curl")
            .args(["-sf", &format!("{}/health", self.base_url)])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn fetch(&self, service: &str, preset: &str) -> Option<String> {
        // GET /secrets/{service}/{preset}
        todo!()
    }
    // ...
}
```

---

## Writing the extended main.rs

Your `main.rs` declares the full CLI (penv commands + yours), delegates to penv for everything shared, and calls your own handlers for new commands.

```rust
// src/main.rs
mod backend;
mod cmd;

use anyhow::Result;
use clap::{Parser, Subcommand};
use penv::{backend::active_backend, backend::SecretBackend as BackendTrait, cmd as pcmd, config::Settings};

fn load_settings() -> (Settings, String) {
    let settings = Settings::load();
    // Read any extra fields your wrapper needs from settings.local.json
    let extra = std::fs::read_to_string(penv::config::settings_file()).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_default();
    let my_token = extra.get("my_vault_token")
        .and_then(|v| v.as_str()).unwrap_or("").to_string();
    (settings, my_token)
}

fn my_active_backend(settings: &Settings, token: &str) -> Option<Box<dyn BackendTrait>> {
    // Check raw setting string for your custom backend
    let raw = std::fs::read_to_string(penv::config::settings_file()).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("secret_backend").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_default();

    match raw.as_str() {
        "sqlite" => {
            let db = penv::config::repo_root().join("vault.db");
            let b = backend::sqlite_vault::SqliteVault::new(db);
            if BackendTrait::available(&b) { return Some(Box::new(b)); }
            None
        }
        "myservice" => {
            let b = backend::http_vault::HttpVault::new("https://secrets.internal", token);
            if BackendTrait::available(&b) { return Some(Box::new(b)); }
            None
        }
        _ => active_backend(settings), // fall through to 1Password
    }
}

#[derive(Parser)]
#[command(name = "myenv", about = "MyProject env manager")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    // ── penv commands (delegate to penv::cmd::*) ────────────────────────────
    LoadEnv { preset: String, service: Option<String> },
    Op { #[command(subcommand)] sub: OpSub },
    MorningCheck,
    Init,
    SetupWizard,
    Export,
    // ... (copy the full list from penv's main.rs)

    // ── your own additions ───────────────────────────────────────────────────
    MyVault {
        #[command(subcommand)]
        sub: MyVaultSub,
    },
}

#[derive(Subcommand)]
enum MyVaultSub {
    Push { #[arg(num_args = 1..)] presets: Vec<String> },
    Pull { #[arg(num_args = 1..)] presets: Vec<String> },
    List,
}

#[derive(Subcommand)]
enum OpSub {
    Push { #[arg(num_args = 1..)] presets: Vec<String>, #[arg(long)] service: Option<String>, #[arg(long)] dry_run: bool },
    Pull { #[arg(num_args = 1..)] presets: Vec<String>, #[arg(long)] service: Option<String>, #[arg(long)] dry_run: bool },
    Diff { preset: String, service: Option<String> },
    List { service: Option<String> },
    InitVault,
    NewPreset,
}

fn main() {
    let cli = Cli::parse();
    let (settings, token) = load_settings();

    let result: Result<()> = match &cli.command {
        Command::LoadEnv { preset, service } =>
            pcmd::load_env::run(preset, service.as_deref(), &settings),
        Command::Op { sub } => match sub {
            OpSub::Push { presets, service, dry_run } =>
                presets.iter().try_fold((), |_, p| pcmd::op_sync::run_push(p, service.as_deref(), &settings, *dry_run)),
            OpSub::Pull { presets, service, dry_run } =>
                presets.iter().try_fold((), |_, p| pcmd::op_sync::run_pull(p, service.as_deref(), &settings, *dry_run)),
            OpSub::Diff { preset, service } => pcmd::op_sync::run_diff(preset, service.as_deref(), &settings),
            OpSub::List { service } => pcmd::op_sync::run_list(service.as_deref(), &settings),
            OpSub::InitVault => pcmd::op_sync::run_init_vault(&settings),
            OpSub::NewPreset => pcmd::op_sync::run_new_preset(&settings),
        },
        Command::MorningCheck => pcmd::morning_check::run(&settings),
        Command::Init => pcmd::init::run(),
        Command::SetupWizard => pcmd::setup_wizard::run(&settings),
        Command::Export => pcmd::export::run(&settings),

        // Your custom backend commands
        Command::MyVault { sub } => {
            let b = my_active_backend(&settings, &token)
                .expect("my-vault backend not available");
            match sub {
                MyVaultSub::Push { presets } =>
                    presets.iter().try_fold((), |_, p| {
                        penv::cmd::sync::cmd_push(p, None, b.as_ref(), &settings, false);
                        Ok(())
                    }),
                MyVaultSub::Pull { presets } =>
                    presets.iter().try_fold((), |_, p| {
                        penv::cmd::sync::cmd_pull(p, None, b.as_ref(), &settings, false);
                        Ok(())
                    }),
                MyVaultSub::List => {
                    penv::cmd::sync::cmd_list(None, b.as_ref(), &settings);
                    Ok(())
                }
            }
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}
```

---

## Project-local binary pattern

For a project where the tool always lives next to the service repos, skip `PENV_REPO_ROOT` entirely:

```
my-project/
  my-project-env/      ← wrapper repo, binary lives here
    myenv               ← built binary
    settings.json
    env/
    local/             ← gitignored
  my-api/
  my-ui/
```

When `my-project-env/myenv` runs, `penv::config::repo_root()` resolves to `my-project-env/` automatically because the binary reads its own path via `std::env::current_exe()`.

Shell scripts in the same dir call `./myenv` and need no environment setup:

```bash
#!/usr/bin/env bash
# load-env.sh
"$(dirname "$0")/myenv" load-env "$@"
```

---

## Keeping your wrapper in sync

When you add a new command to `project-env-utils`:

1. Add the new `pub fn` to the relevant `cmd/*.rs` module
2. In your wrapper's `main.rs`, add the variant to your `Command` enum
3. Wire up the one-line match arm that calls `pcmd::new_command::run(...)`

Since penv is a path dependency, `cargo build` picks up the change immediately — no version bump or publish needed.

---

## settings.local.json for custom backends

Add your backend's config fields to `settings.local.json` (gitignored):

```json
{
  "secret_backend": "sqlite",
  "my_vault_token": "tok_abc123"
}
```

Read them in your `load_settings()` function alongside penv's standard fields. Nothing in penv's `Settings::load()` will error on unknown keys — they're simply ignored.
