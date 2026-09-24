# Changelog

All notable changes to project-env-utils are documented here.

## [0.3.0] - 2026-09-24

### Added
- **`workspace` command** (`penv workspace open / wizard / guide`) — generates VS Code `.code-workspace` files with worktree-aware folder paths, Peacock color picker, per-group branch overrides (`--group ui=branch`), and named preset support
- **`dev-ui-ext` / `dev-backend-ext`** — VS Code integration wrappers: prompt whether to open a workspace and which worktree to use before starting the tmux session; `DevUi` and `DevBackend` now route through these by default
- **`local_dir` top-level setting** in `settings.json` — overrides the default `local/<project_name>` preset store with a flat `<local_dir>/<service>/<preset>.env` path; accepts absolute or relative paths
- **`title` field on `GridPaneConfig` and `SessionPaneConfig`** — sets the tmux pane border title via `select-pane -T` and `@pane_name` user option; falls back to `repo` when absent
- **`dev-session` workspace flags** (`--worktree`, `--checkout`, `--checkout-worktree`, `--branch`, `--mixed`) — resolve each repo's working directory per-mode before launching panes; `--mixed` prompts interactively
- **`WorkspaceMode` enum** + `pick_workspace_mode()`, `mode_from_flags()`, `resolve_repo_workspace()` in `worktree.rs` — shared primitives for consistent workspace mode handling across commands
- **`change_preset`** now shows `(current)` marker next to the active preset
- **`list_presets`** command wired into the penv CLI (`penv list-presets [service]`)
- **`PENV_REPO_ROOT` injection** in all tmux session helper scripts and `show-info` calls — pane commands find `settings.json` regardless of which directory the pane starts in
- **Pane title persistence** via `tmux set-option -p -t <pane> @pane_name <title>` — immune to shell `preexec` overrides; pair with `pane-border-format " #{?#{@pane_name},#{@pane_name},#T} "` in `tmux.conf`
- **`dev-session` info bar** — always shown; creates a dedicated `shell` window when all configured grid positions are filled
- **Demo** fully configured: `start.sh`, `load-env.sh`, `dev-ui.sh`, `dev-backend.sh`, `dev-session.sh`, `start-vscode.sh`; `dev` and `test` presets via `local_dir=env`; pane titles throughout

### Fixed
- **`change_preset` / `list_presets` / `validate_cache`** — now call `available_presets_for_service()` so the `local_dir` override is respected everywhere
- **`dev-session` info bar** missing when all pane positions were occupied — falls back to a new `shell` window
- **`PENV_REPO_ROOT` not inherited by tmux panes** — injected as `PENV_REPO_ROOT='...'` prefix in `show-info` calls and as `export PENV_REPO_ROOT=...` in helper scripts
- **`tmux set-option` flag syntax** — was `-pt` (single argument); corrected to `-p -t` (separate flags) so `@pane_name` is actually stored

### Changed
- `penv dev-ui` and `penv dev-backend` now open a VS Code workspace prompt before starting the tmux session (behaviour added by the ext wrappers; pass `--attach` to skip)
- `settings.schema.json`: added `local_dir`, `title`; removed stale `pane_dev_cmd` / `pane_sb_cmd` from `dev_ui`


### Added
- **`validate-cache`** command — diffs every `local/<project>/<service>/<preset>.env` against the active backend (SQLite by default); exits non-zero if anything is out of sync
- **`validate-cache --fix`** — pushes local cache → backend for differing pairs, backing up the old backend content to `backups/<ts>-validate-cache-fix/` first
- **`no_attach`** field on `DevBackendArgs` and `DevUiArgs` — allows callers to start a session without exec-ing into it (used by the combined launcher in project-specific wrappers)

### Fixed
- **`resolve_backend_preset`** (root checkout) now skips auto-loading `.env` when the repo's `.env` is newer than the local cache — matching the existing behaviour of `resolve_workspace_preset` for worktrees; `startup_sync_check` then handles the diff interactively
- **Session-exists guard** in `dev_backend::run()` and `dev_ui::run()` now respects `no_attach` — returns `Ok(())` instead of exec-ing when the caller manages attaching

## [0.2.3] - 2026-08-07

### Added
- **`--branch <name>` implies `--checkout`** in `dev-ui` and `dev-backend` — shorthand that stashes and checks out a branch without requiring the explicit `--checkout` flag
- **`dev-backend --checkout` prompts per-repo** when `--branch` is not given — each repo in `window_backend` gets its own fzf branch picker, enabling independent branch selection (e.g. `api` and `apps-api` can land on different branches)

## [0.2.2] - 2026-07-16

### Added
- **`open-pr` command** (`penv open-pr [--branch <name>]`) — finds the GitHub PR for the current (or specified) branch via `gh pr view`; exits with an error when no PR exists; no configuration required
- **`open_pr()` shell function** injected into every dev session helper script alongside `open_ticket`
- `open_pr — open GitHub PR in browser` always shown in the `show_info` box (unconditional, unlike `open_ticket`)
- `DevUiArgs.worktree_path: Option<PathBuf>` — callers can pre-select a worktree to bypass the interactive picker inside `run()`
- `DevBackendArgs.worktree_branch: Option<String>` — same bypass for the backend worktree branch picker
- `validate` command (`penv validate`) — validates `settings.json` against its declared `$schema` path

### Fixed
- `settings.schema.json`: added `$schema` as an allowed top-level property (fixes VS Code "property $schema is not allowed")
- `settings.schema.json`: added `panes` array to `dev_ui`; removed `pane_git_cmd` from `dev_backend`
- Clippy: `map_or(false, …)` → `is_some_and(…)`, `#[derive(Default)]` on `SessionNotes`, `.last()` → `.next_back()` on `DoubleEndedIterator`, unused `_parent` bindings

### Removed
- `DevBackendConfig.pane_git_cmd` and the auto-generated git window in `dev-backend` — configure lazygit panes directly in the `sessions` block instead

## [0.2.1] - 2026-07-16

### Added

- **`open-ticket` command** (`penv open-ticket [--branch <name>]`) — extracts a ticket ID from the current git branch using a configurable regex pattern and opens the configured URL template in the default browser; errors when `ticket_manager` is not configured or no match is found
- **`ticket_manager` config block** in `settings.json`:
  - `pattern` — regex for ticket ID extraction (e.g. `\d+` for Wrike, `[A-Z]+-\d+` for Jira)
  - `url_template` — URL with `{ticket}` placeholder
- **`open_ticket()` shell function** injected into every dev session helper script alongside `reload_env`, `change_preset`, etc.
- **`SessionNotes` builder** in `cmd::show_info` — replaces hardcoded column-aligned note strings with a typed `Vec`-backed builder; `add_if(condition, name, desc)` enables dynamic entries; `to_show_info_args()` auto-aligns name columns
- Dev session info box (`show_info`) now shows `open_ticket` only when the current branch matches the configured pattern — omitted entirely when no ticket is present or `ticket_manager` is unconfigured

### Changed

- `dev_ui`, `dev_backend`, `dev_session`: replaced hardcoded `--notes` format strings with `SessionNotes` builder (no behaviour change for existing configurations)

## [0.2.0] - 2026-07-14

### Breaking changes

- `dev-ui` / `dev-backend`: `--select` renamed to `--worktree`
- `dev-ui` / `dev-backend`: `--resume [branch]` renamed to `--checkout-worktree --branch <name>`
- Local env file layout changed from flat `local/<project>/<service>.<preset>.env` to per-service subfolders `local/<project>/<service>/<preset>.env`; generic fallback renamed from `<service>.env` to `local.env`; shared managed files now live under `local/<project>/shared/`
- `env/` split-profile directories removed — `local/` is now the single source of truth (no secret backend required for basic use)

### New features

#### `change-preset` command
Interactive preset switcher — shows a numbered menu of available presets, prompts for a selection, and reloads `.env` files for all (or specified) services. Exposed as a `change_preset()` shell function in every dev session alongside `reload_env`.

#### `--checkout` flag (`dev-ui`, `dev-backend`)
Opens the session in the root repos at a selected (or newly created) branch. Stashes uncommitted changes before switching, reports the stashed file list and stash reference, and creates the branch if it does not yet exist locally.

#### `--checkout-worktree` flag (`dev-ui`, `dev-backend`) · alias `--worktree-checkout`
Creates or resumes a Claude worktree for a branch. Errors immediately if the requested branch is already checked out in the root repo, directing the user to `--checkout` instead. Creates the branch if it does not yet exist.

#### New branch creation
`--checkout` and `--checkout-worktree` now accept branch names that don't exist yet — no need to create the branch manually first:
- `stash_and_checkout`: falls back to `git checkout -b` when the branch is unknown
- `ensure_worktree`: falls back to `git worktree add -b` when the branch is unknown
- `pick_branch_fzf`: uses fzf `--print-query` so typing a new name and pressing Enter returns it directly

#### Informative auto-stash output
When `--checkout` triggers a stash, the output lists every changed file (from `git status --porcelain`) and prints the stash ref git created. The stash label encodes the source branch and wall-clock time: `penv: <branch> @ YYYY-MM-DD HH:MM:SS`, making `git stash list` immediately readable.

#### Colored diffs by default
`morning_check::show_diff` now respects `resolve_use_color` instead of being hardcoded to `false`. Diffs shown during startup sync-checks are colored the same way as explicit `diff` subcommands. Opt out with `--no-color`, `NO_COLOR`, or `"diff_color": false` in `settings.local.json`.

### Improvements

- `available_presets` reads from `local/<project>/<service>/` instead of the removed `env/<service>/` directories
- `pick_branch_fzf` prompt updated to `Branch (new or existing):` to signal free-text input is accepted
- `pick_existing_worktree_fzf` error message updated to reference `--checkout-worktree`
- New helpers `current_branch()` and `stash_and_checkout()` extracted to `worktree.rs` for reuse
- Edition bumped to **2024**; `env::set_var` / `env::remove_var` wrapped in `unsafe` blocks; test helpers `set_repo_root` / `clear_repo_root` added to `test_utils`
- All `collapsible_if` clippy lints fixed using `if let` chains (Rust 2024 stabilised feature)
- Two load_env tests updated to reflect the new subfolder path layout

## Unreleased

### Added
- **SQLite backend** — new `secret_backend = "sqlite"` option stores all env secrets in a local `local/secrets.db` file. No external CLI required. Useful for offline use or as a lightweight alternative to 1Password.
- **Demo project** — `demo/` directory with a complete working example showing how to set up a new project and use the SQLite backend.

### Changed
- **Local cache is now folder-per-project** — env cache files move from `local/<service>.<preset>.env` to `local/<project_name>/<service>.<preset>.env`. Prevents collisions when multiple projects share the same `penv` binary directory.
- **Config is now required** — `penv` exits with a clear error if `settings.json` is missing or has no `project.name`. Previously it silently returned defaults; now it surfaces the misconfiguration early.

## [0.1.0] - Initial release

### Added
- Core env preset management: load, push, pull, diff across presets and services
- 1Password backend (`op push/pull/diff/list`) with per-service Secure Note items
- `morning-check` — interactive start-of-day sync verification
- `env-age` — modification time comparison across service repo, local cache, and backend
- `dev-ui`, `dev-backend`, `dev-session` — tmux session launchers with preset resolution
- `pick-preset` — interactive preset selector for shell script integration
- `export` — export presets to a folder or zip archive
- `init` / `setup-wizard` — guided first-time setup
- Backup reflog: every env mutation is recorded with timestamp and reason
- State tracking in `.state/active.json` for per-workspace and per-service preset recall


### Added
- **SQLite backend** — new `secret_backend = "sqlite"` option stores all env secrets in a local `local/secrets.db` file. No external CLI required. Useful for offline use or as a lightweight alternative to 1Password.
- **Demo project** — `demo/` directory with a complete working example showing how to set up a new project and use the SQLite backend.

### Changed
- **Local cache is now folder-per-project** — env cache files move from `local/<service>.<preset>.env` to `local/<project_name>/<service>.<preset>.env`. Prevents collisions when multiple projects share the same `penv` binary directory.
- **Config is now required** — `penv` exits with a clear error if `settings.json` is missing or has no `project.name`. Previously it silently returned defaults; now it surfaces the misconfiguration early.

## 0.1.0 — Initial release

### Added
- Core env preset management: load, push, pull, diff across presets and services
- 1Password backend (`op push/pull/diff/list`) with per-service Secure Note items
- `morning-check` — interactive start-of-day sync verification
- `env-age` — modification time comparison across service repo, local cache, and backend
- `dev-ui`, `dev-backend`, `dev-session` — tmux session launchers with preset resolution
- `pick-preset` — interactive preset selector for shell script integration
- `export` — export presets to a folder or zip archive
- `init` / `setup-wizard` — guided first-time setup
- Backup reflog: every env mutation is recorded with timestamp and reason
- State tracking in `.state/active.json` for per-workspace and per-service preset recall
