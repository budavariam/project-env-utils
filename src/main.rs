/// penv — dev environment utilities.
mod backend;
mod backup;
mod cmd;
mod config;
mod env_file;
mod state;
mod tmux;

/// Shared test utilities — compiled only during `cargo test`.
#[cfg(test)]
pub mod test_utils {
    use std::sync::Mutex;
    /// Single process-wide lock for tests that mutate PENV_REPO_ROOT.
    pub static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Set PENV_REPO_ROOT in tests. Safe because all tests hold ENV_LOCK.
    pub fn set_repo_root(path: &str) {
        // SAFETY: tests serialize env mutation via ENV_LOCK.
        unsafe { std::env::set_var("PENV_REPO_ROOT", path) }
    }

    /// Remove PENV_REPO_ROOT after a test.
    pub fn clear_repo_root() {
        // SAFETY: tests serialize env mutation via ENV_LOCK.
        unsafe { std::env::remove_var("PENV_REPO_ROOT") }
    }
}

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "penv", about = "Dev environment utilities", version)]
struct Cli {
    /// Path to the penv config directory (the one containing settings.json).
    /// Overrides auto-detection from the current directory / git root.
    #[arg(long, global = true)]
    config: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Load a preset's .env into service repos
    LoadEnv {
        preset: String,
        service: Option<String>,
    },

    /// Interactive preset selector — stdout: PRESET:<name>
    PickPreset {
        #[arg(long, group = "mode")]
        workspace: Option<String>,
        #[arg(long, group = "mode")]
        backend: bool,
        #[arg(long)]
        preset: Option<String>,
    },

    /// Print a Unicode session summary box
    ShowInfo {
        #[arg(long, required = true)]
        preset: String,
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long, num_args = 1..)]
        services: Option<Vec<String>>,
        /// Custom message displayed at the bottom of the info box (word-wrapped).
        #[arg(long)]
        message: Option<String>,
        #[arg(long, num_args = 1..)]
        notes: Option<Vec<String>>,
    },

    /// 1Password sync subcommands
    Op {
        #[command(subcommand)]
        sub: OpSub,
    },

    /// Interactive start-of-day sync check
    MorningCheck,

    /// Interactively switch to a different preset and reload .env files
    ChangePreset {
        /// Services to reload (defaults to all configured services)
        #[arg(num_args = 0..)]
        services: Vec<String>,
    },

    /// Launch the ui tmux dev session
    DevUi {
        #[arg(long)]
        preset: Option<String>,
        /// Open the session in an existing Claude worktree
        #[arg(long, group = "mode")]
        worktree: bool,
        /// Checkout a branch in the root repo (stash first), open session there
        #[arg(long, group = "mode")]
        checkout: bool,
        /// Checkout a branch into a new/existing Claude worktree
        #[arg(long, alias = "worktree-checkout", group = "mode")]
        checkout_worktree: bool,
        /// Branch for --checkout or --checkout-worktree (prompts if omitted)
        #[arg(long)]
        branch: Option<String>,
        /// Attach to an existing dev-ui session instead of creating a new one
        #[arg(long)]
        attach: bool,
    },

    /// Interactive first-time setup — creates settings.json from scratch
    Init,

    /// Interactive 1Password setup wizard
    SetupWizard,

    /// Export presets to service repos — prompts for preset per service
    Export,

    /// Launch the backend tmux dev session
    DevBackend {
        #[arg(long)]
        preset: Option<String>,
        /// Open the session in an existing Claude worktree
        #[arg(long, group = "mode")]
        worktree: bool,
        /// Checkout a branch in the root repos (stash first), open session there
        #[arg(long, group = "mode")]
        checkout: bool,
        /// Checkout a branch into a new/existing Claude worktrees
        #[arg(long, alias = "worktree-checkout", group = "mode")]
        checkout_worktree: bool,
        /// Branch for --checkout or --checkout-worktree (prompts if omitted)
        #[arg(long)]
        branch: Option<String>,
        /// Attach to an existing dev-backend session instead of creating a new one
        #[arg(long)]
        attach: bool,
    },

    /// Launch a custom grid session defined in settings.json under 'sessions'
    DevSession {
        /// Session name (required when more than one session is configured)
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        preset: Option<String>,
        /// Attach to an existing session instead of creating a new one
        #[arg(long)]
        attach: bool,
    },

    /// Show env file ages across service repo, local cache, and backend
    EnvAge {
        #[arg(long)]
        service: Option<String>,
        #[arg(long)]
        preset: Option<String>,
    },
}

#[derive(Subcommand)]
enum OpSub {
    Push {
        #[arg(num_args = 1..)]
        presets: Vec<String>,
        #[arg(long)]
        service: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    Pull {
        #[arg(num_args = 1..)]
        presets: Vec<String>,
        #[arg(long)]
        service: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    Diff {
        preset: String,
        service: Option<String>,
        #[arg(long)]
        no_color: bool,
    },
    List {
        service: Option<String>,
    },
    /// Verify (and optionally create) the 1Password vault
    InitVault,
    /// Interactive: copy an existing preset to a new name
    NewPreset,
}

fn main() {
    let cli = Cli::parse();

    // Apply --config / PENV_REPO_ROOT before loading settings so all path
    // helpers in config.rs see the override via the env var.
    if let Some(ref dir) = cli.config {
        // SAFETY: single-threaded startup; no other threads read PENV_REPO_ROOT yet.
        unsafe { std::env::set_var("PENV_REPO_ROOT", dir) };
    }

    let settings = config::Settings::load().unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    });

    let result = match &cli.command {
        Command::LoadEnv { preset, service } => {
            cmd::load_env::run(preset, service.as_deref(), &settings)
        }

        Command::PickPreset {
            workspace,
            backend: use_backend,
            preset,
        } => {
            use cmd::pick_preset;
            let backend_box = crate::backend::active_backend(&settings);
            let bref = backend_box.as_deref();
            if let Some(ws) = workspace {
                pick_preset::run_workspace(ws, preset.as_deref(), bref, &settings)
            } else if *use_backend {
                pick_preset::run_backend(preset.as_deref(), bref, &settings)
            } else {
                eprintln!("Error: one of --workspace or --backend is required");
                std::process::exit(1);
            }
        }

        Command::ShowInfo {
            preset,
            workspace,
            services,
            message,
            notes,
        } => cmd::show_info::run(
            preset,
            workspace.as_deref(),
            services.as_deref(),
            message.as_deref(),
            notes.as_deref(),
            &settings,
        ),

        Command::Op { sub } => match sub {
            OpSub::Push {
                presets,
                service,
                dry_run,
            } => presets.iter().try_fold((), |_, p| {
                cmd::op_sync::run_push(p, service.as_deref(), &settings, *dry_run)
            }),
            OpSub::Pull {
                presets,
                service,
                dry_run,
            } => presets.iter().try_fold((), |_, p| {
                cmd::op_sync::run_pull(p, service.as_deref(), &settings, *dry_run)
            }),
            OpSub::Diff {
                preset,
                service,
                no_color,
            } => cmd::op_sync::run_diff(preset, service.as_deref(), &settings, *no_color),
            OpSub::List { service } => cmd::op_sync::run_list(service.as_deref(), &settings),
            OpSub::InitVault => cmd::op_sync::run_init_vault(&settings),
            OpSub::NewPreset => cmd::op_sync::run_new_preset(&settings),
        },

        Command::MorningCheck => cmd::morning_check::run(&settings),

        Command::ChangePreset { services } => cmd::change_preset::run(services, &settings),

        Command::Init => cmd::init::run(),

        Command::SetupWizard => cmd::setup_wizard::run(&settings),

        Command::Export => cmd::export::run(&settings),

        Command::DevUi {
            preset,
            worktree,
            checkout,
            checkout_worktree,
            branch,
            attach,
        } => cmd::dev_ui::run(
            &cmd::dev_ui::DevUiArgs {
                preset: preset.clone(),
                worktree: *worktree,
                checkout: *checkout,
                checkout_branch: if *checkout { branch.clone() } else { None },
                checkout_worktree: *checkout_worktree,
                checkout_worktree_branch: if *checkout_worktree {
                    branch.clone()
                } else {
                    None
                },
                attach: *attach,
            },
            &settings,
        ),

        Command::DevBackend {
            preset,
            worktree,
            checkout,
            checkout_worktree,
            branch,
            attach,
        } => cmd::dev_backend::run(
            &cmd::dev_backend::DevBackendArgs {
                preset: preset.clone(),
                worktree: *worktree,
                checkout: *checkout,
                checkout_branch: if *checkout { branch.clone() } else { None },
                checkout_worktree: *checkout_worktree,
                checkout_worktree_branch: if *checkout_worktree {
                    branch.clone()
                } else {
                    None
                },
                attach: *attach,
            },
            &settings,
        ),

        Command::DevSession {
            session,
            preset,
            attach,
        } => cmd::dev_session::run(
            &cmd::dev_session::DevSessionArgs {
                session_name: session.clone(),
                preset: preset.clone(),
                attach: *attach,
            },
            &settings,
        ),

        Command::EnvAge { service, preset } => {
            let backend_box = crate::backend::active_backend(&settings);
            let bref = backend_box.as_deref();
            let active_preset = preset
                .as_deref()
                .map(|s| s.to_string())
                .or_else(|| {
                    service
                        .as_deref()
                        .and_then(|svc| crate::state::State::load().services.get(svc).cloned())
                })
                .unwrap_or_else(|| "dev".to_string());
            cmd::env_age::run(service.as_deref(), &active_preset, bref, &settings)
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}
