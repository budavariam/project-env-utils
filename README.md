# project-env-utils

Preset-based `.env` file manager and tmux session launcher for multi-repo projects.

Keeps your environment files in sync across service repos and lets you switch presets (`dev`, `test`, `uat`, …) with a single command. Optionally syncs with 1Password. Ships as both a Rust library (embed in your own project-specific binary) and a standalone `penv` CLI.

## Concept

Each `(service, preset)` pair maps to one complete `.env` file. Penv loads, syncs, and manages those files — and launches configured tmux dev sessions around them.

```
local/<project>/<service>/<preset>.env    — preset-specific env file
local/<project>/<service>/local.env       — generic fallback (any preset)
local/<project>/shared/<label>            — shared file (keys, certs, …)
```

Alternatively, set `local_dir` in `settings.json` to use a different directory
(e.g. a tracked `env/` folder for demo projects with no real secrets).

## Quick start

```bash
# 1. Build
cargo build --release
cp target/release/penv /usr/local/bin/penv

# 2. Point penv at your config
export PENV_REPO_ROOT=/path/to/your/penv-config-dir

# 3. First-time setup
penv init          # creates settings.json interactively

# 4. Load env into all service repos
penv load-env dev
```

See [docs/setup.md](docs/setup.md) for a full walkthrough.

## Secret backends

| Backend | Default | How to enable |
|---|---|---|
| **SQLite** | ✓ | Always active — `local/secrets.db` (gitignored) |
| **1Password** | — | `"secret_backend": "onepassword"` in `settings.local.json` |

SQLite stores all `(project, service, preset)` content locally in a single database file. It enables `validate-cache` and `morning-check` without any external account. 1Password can be layered on top for team sharing.

## Commands

### Environment management

| Command | Description |
|---|---|
| `penv init` | Guided setup — creates `settings.json` |
| `penv load-env <preset> [service]` | Write preset env into one or all service repos |
| `penv change-preset [services…]` | Interactively switch preset and reload `.env` files |
| `penv list-presets [service]` | List available preset names (intersection across all services when no service given) |
| `penv export` | Export presets to a folder |
| `penv morning-check` | Start-of-day sync check against the active backend |
| `penv env-age [--service S] [--preset P]` | Show env file ages across repo, local cache, and backend |
| `penv validate-cache [--fix]` | Diff local cache vs backend; `--fix` pushes local → backend |
| `penv validate` | Validate `settings.json` against its JSON Schema |

### 1Password sync

| Command | Description |
|---|---|
| `penv op pull <preset> …` | Pull from 1Password |
| `penv op push <preset> …` | Push to 1Password |
| `penv op diff <preset>` | Compare local vs 1Password (colored) |
| `penv op list` | List presets in 1Password |
| `penv op new-preset` | Copy a preset to a new name interactively |
| `penv op init-vault` | Verify (or create) the configured vault |
| `penv setup-wizard` | Guided 1Password configuration |

### Dev sessions

| Command | Description |
|---|---|
| `penv dev-ui [flags]` | Launch UI tmux session (prompts to open VS Code workspace) |
| `penv dev-backend [flags]` | Launch backend tmux session (prompts to open VS Code workspace) |
| `penv dev-session [flags]` | Launch a custom grid session defined in `sessions[]` |
| `penv open-ticket [--branch B]` | Open the ticket URL extracted from the current branch |
| `penv open-pr [--branch B]` | Open the GitHub PR for the current branch |

#### Workspace flags (all three dev commands)

| Flag | Behaviour |
|---|---|
| _(none)_ | Open repos at their current root checkout |
| `--checkout [--branch <name>]` | Stash + checkout branch in root repos |
| `--worktree` | Pick an existing Claude worktree via fzf |
| `--checkout-worktree [--branch <name>]` | Create/open a Claude worktree for the branch |
| `--mixed` | Prompt interactively for a workspace mode |
| `--attach` | Reconnect to an already-running session |

Each session pane gets shell functions: `reload_env`, `change_preset`, `close_session`, `show_info`, `inspect_env`, `open_ticket`, `open_pr`.

### VS Code workspace

| Command | Description |
|---|---|
| `penv workspace open [flags]` | Generate and open a `.code-workspace` file |
| `penv workspace wizard [--save <name>]` | fzf multiselect repos, optionally save as preset |
| `penv workspace guide` | Step-by-step interactive guide |

`workspace open` flags: `--preset <name>`, `--branch <B>`, `--worktree`, `--group <name>=<branch>`, `--peacock-color <hex>`, `--ff`, `--no-open`.

### Utility

| Command | Description |
|---|---|
| `penv show-info` | Print a Unicode session info box |
| `penv pick-preset` | Interactive preset selector (stdout: `PRESET:<name>`) |

## Configuration (`settings.json`)

```jsonc
{
  "project": {
    "name": "my-project",          // used as namespace for local cache and SQLite
    "op_vault": "My Vault",        // 1Password vault (overridable in settings.local.json)
    "op_item_prefix": "myproj"     // prefix for 1Password item names
  },
  "local_dir": "env",              // optional: override 'local/<project>' preset store
                                   // path is relative to settings.json or absolute
                                   // flat structure: <local_dir>/<service>/<preset>.env
  "services": [
    {
      "name": "my-api",
      "env_vars": ["NODE_ENV", "DATABASE_URL"],
      "files": [{ "label": "api-key", "path": "keys/api.key", "shared": true }]
    }
  ],
  "dev_ui": {
    "repo": "my-ui",
    "panes": [
      { "col": 0, "row": 0, "cmd": "npm run dev", "title": "dev" },
      { "col": 1, "row": 0, "cmd": "lazygit",    "title": "git" }
    ]
  },
  "dev_backend": {
    "session_name": "my-backend",
    "window_backend": [
      { "repo": "my-api", "cmd": "npm run dev", "title": "api" }
    ],
    "window_service": []
  },
  "sessions": [
    {
      "session_name": "my-dev",
      "tabs": [
        {
          "name": "app",
          "panes": [
            { "repo": "my-ui",  "cmd": "npm run dev", "col": 0, "row": 0, "title": "ui" },
            { "repo": "my-api", "cmd": "npm run dev", "col": 1, "row": 0, "title": "api" }
          ]
        }
      ]
    }
  ],
  "vscode": {
    "output_file": "my-project.code-workspace",
    "peacock_color": "#3b82f6",
    "extra_settings": { "github-enterprise.uri": "https://github.example.com" },
    "presets": [{ "name": "simple", "folders": ["my-ui", "my-api"] }]
  },
  "ticket_manager": {
    "pattern": "\\d+",
    "url_template": "https://your-tracker.example.com/issues/{ticket}"
  }
}
```

### Pane titles

Set `"title"` on any pane to display a fixed label in the tmux border:

```jsonc
{ "repo": "my-api", "cmd": "npm run dev", "title": "api" }
```

Titles are stored as tmux user pane options (`@pane_name`) — immune to shell `preexec` overrides. Enable in `~/.tmux.conf`:

```
set -g pane-border-status top
set -g pane-border-format " #{?#{@pane_name},#{@pane_name},#T} "
```

### `local_dir` override

When secrets are checked in (e.g. a demo project), point `local_dir` at the tracked folder:

```jsonc
{
  "local_dir": "env"   // reads from env/<service>/<preset>.env instead of local/<project>/…
}
```

## Local file layout (default)

```
local/                         ← gitignored
  <project>/
    <service>/
      dev.env
      test.env
      local.env                ← generic fallback
    shared/
      signing.key
  secrets.db                   ← SQLite backend database
```

## Embedding as a library

Add to `Cargo.toml`:

```toml
[dependencies]
penv = { path = "../project-env-utils" }
# or as a git submodule:
penv = { path = "project-env-utils" }
```

Override `PENV_REPO_ROOT` at startup to point at your project's config directory, then call any `penv::cmd::*` entry point. See [docs/extending.md](docs/extending.md) for a complete example.

## Documentation

- [Setup guide](docs/setup.md) — install, first-time configuration, daily workflow
- [Configuration reference](docs/configuration.md) — all `settings.json` fields
- [Extending penv](docs/extending.md) — wrapping penv as a library

## License

MIT
