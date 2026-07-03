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
    /// All modules must acquire THIS lock, not their own copy, so that tests
    /// from different modules don't race when running in parallel.
    pub static ENV_LOCK: Mutex<()> = Mutex::new(());
}

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "penv", about = "Dev environment utilities", version)]
struct Cli {
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

    /// Launch the ui tmux dev session
    DevUi {
        #[arg(long)]
        preset: Option<String>,
        #[arg(long)]
        select: bool,
        #[arg(long)]
        resume: bool,
        #[arg(requires = "resume")]
        resume_branch: Option<String>,
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
        /// Pick an existing branch via fzf
        #[arg(long)]
        select: bool,
        /// Resume (or create) a worktree at the given branch
        #[arg(long)]
        resume: bool,
        /// Branch name for --resume (optional, prompts if omitted)
        #[arg(requires = "resume")]
        resume_branch: Option<String>,
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
    Diff { preset: String, service: Option<String> },
    List { service: Option<String> },
    /// Verify (and optionally create) the 1Password vault
    InitVault,
    /// Interactive: copy an existing preset to a new name
    NewPreset,
}

fn main() {
    let cli = Cli::parse();
    let settings = config::Settings::load();

    let result = match &cli.command {
        Command::LoadEnv { preset, service } => {
            cmd::load_env::run(preset, service.as_deref(), &settings)
        }

        Command::PickPreset { workspace, backend: use_backend, preset } => {
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

        Command::ShowInfo { preset, workspace, services, notes } => {
            cmd::show_info::run(
                preset,
                workspace.as_deref(),
                services.as_deref(),
                notes.as_deref(),
                &settings,
            )
        }

        Command::Op { sub } => match sub {
            OpSub::Push { presets, service, dry_run } => {
                presets.iter().try_fold((), |_, p| cmd::op_sync::run_push(p, service.as_deref(), &settings, *dry_run))
            }
            OpSub::Pull { presets, service, dry_run } => {
                presets.iter().try_fold((), |_, p| cmd::op_sync::run_pull(p, service.as_deref(), &settings, *dry_run))
            }
            OpSub::Diff { preset, service } => {
                cmd::op_sync::run_diff(preset, service.as_deref(), &settings)
            }
            OpSub::List { service } => cmd::op_sync::run_list(service.as_deref(), &settings),
            OpSub::InitVault => cmd::op_sync::run_init_vault(&settings),
            OpSub::NewPreset => cmd::op_sync::run_new_preset(&settings),
        },

        Command::MorningCheck => cmd::morning_check::run(&settings),

        Command::Init => cmd::init::run(),

        Command::SetupWizard => cmd::setup_wizard::run(&settings),

        Command::Export => cmd::export::run(&settings),

        Command::DevUi { preset, select, resume, resume_branch, attach } => cmd::dev_ui::run(
            &cmd::dev_ui::DevUiArgs {
                preset: preset.clone(),
                select: *select,
                resume: *resume,
                resume_branch: resume_branch.clone(),
                attach: *attach,
            },
            &settings,
        ),

        Command::DevBackend { preset, select, resume, resume_branch, attach } => cmd::dev_backend::run(
            &cmd::dev_backend::DevBackendArgs {
                preset: preset.clone(),
                select: *select,
                resume: *resume,
                resume_branch: resume_branch.clone(),
                attach: *attach,
            },
            &settings,
        ),

        Command::DevSession { session, preset, attach } => cmd::dev_session::run(
            &cmd::dev_session::DevSessionArgs {
                session_name: session.clone(),
                preset: preset.clone(),
                attach: *attach,
            },
            &settings,
        ),
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}
