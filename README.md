# project-env-utils

Preset-based `.env` file manager for multi-repo projects.

Keeps your environment files in sync with [1Password](https://1password.com) or a local cache and lets you switch presets (`test`, `uat`, `dev`, …) across all service repos with a single command.

## Concept

Each `(service, preset)` pair maps to one complete `.env` file. Files are stored under:

```
local/<project>/<service>/<preset>.env    — preset-specific
local/<project>/<service>/local.env       — generic fallback (any preset)
local/<project>/shared/<label>            — shared managed files
```

`penv load-env test` writes the matching `.env` into every service repo simultaneously.

When a secret backend (1Password / Whisper / SQLite) is configured, penv first looks there; local files are the fallback.

## Quick start

```bash
# 1. Build and install
cargo build --release
cp target/release/penv /usr/local/bin/penv

# 2. Point penv at your project
export PENV_REPO_ROOT=/path/to/your/project-env-dir

# 3. Guided first-time setup
penv init          # creates settings.json
penv setup-wizard  # configures 1Password (optional)

# 4. Load env into service repos
penv load-env test
```

See [docs/setup.md](docs/setup.md) for a complete walkthrough.

## Commands

| Command | Description |
|---|---|
| `penv init` | Guided setup — creates `settings.json` |
| `penv setup-wizard` | Configure 1Password as the secret backend |
| `penv export` | Export preset(s) to a folder, optionally zip |
| `penv load-env <preset>` | Load a preset into service repos |
| `penv change-preset` | Interactively switch to a different preset and reload .env files |
| `penv morning-check` | Start-of-day sync check |
| `penv op pull <preset> …` | Pull one or more presets from 1Password |
| `penv op push <preset> …` | Push one or more presets to 1Password |
| `penv op new-preset` | Interactively copy a preset to a new name |
| `penv op diff <preset>` | Compare local .env files vs 1Password (colored by default) |
| `penv op list` | List all presets in 1Password |
| `penv dev-ui [flags]` | Launch a configured tmux UI session |
| `penv dev-backend [flags]` | Launch a configured tmux backend session |
| `penv dev-session` | Launch a flexible grid session defined in `sessions[]` |

### Dev session flags (`dev-ui`, `dev-backend`)

| Flag | Behaviour |
|---|---|
| _(none)_ | Open session in root repos at their current branch |
| `--checkout [--branch <name>]` | Stash changes, checkout branch in root repos (creates branch if new) |
| `--worktree` | Pick an existing Claude worktree via fzf |
| `--checkout-worktree [--branch <name>]` | Checkout branch into a Claude worktree (creates branch if new; errors if branch is root's HEAD) |
| `--attach` | Reconnect to an already-running session |

Each session pane exposes: `reload_env`, `change_preset`, `close_session`, `show_info`, `inspect_env`.

## Local file layout

```
local/
  <project>/
    <service>/
      dev.env          ← preset-specific env file
      test.env
      local.env        ← generic fallback (any preset)
    shared/
      some-key.pem     ← shared across services
```

## Documentation

- [Setup guide](docs/setup.md) — install, first-time configuration, daily workflow
- [Configuration reference](docs/configuration.md) — `settings.json` fields, file layout, path resolution
- [Extending penv](docs/extending.md) — how to wrap penv as a library to add project-specific backends and commands

## License

MIT
