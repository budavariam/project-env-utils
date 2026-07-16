# Changelog

All notable changes to project-env-utils are documented here.

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
