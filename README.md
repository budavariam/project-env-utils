# project-env-utils

Preset-based `.env` file manager for multi-repo projects.

Keeps your environment files in sync with [1Password](https://1password.com) and lets you switch presets (`test`, `uat`, `prod`, …) across all service repos with a single command.

## Concept

Each `(service, preset)` pair maps to one complete `.env` file stored in 1Password. For example:

```
my-api  × test  →  myproject/my-api  §  test
my-api  × uat   →  myproject/my-api  §  uat
my-ui   × test  →  myproject/my-ui   §  test
```

`penv op pull test` writes the matching `.env` into every service repo simultaneously. No more per-developer secret spreadsheets.

## Quick start

```bash
# 1. Build and install
cargo build --release
cp target/release/penv /usr/local/bin/penv

# 2. Point penv at your project
export PENV_REPO_ROOT=/path/to/your/project-env-dir

# 3. Guided first-time setup
penv init          # creates settings.json
penv setup-wizard  # configures 1Password

# 4. Push existing .env files to 1Password
penv op push test

# 5. On another machine
penv op pull test
```

See [docs/setup.md](docs/setup.md) for a complete walkthrough.

## Commands

| Command | Description |
|---|---|
| `penv init` | Guided setup — creates `settings.json` |
| `penv setup-wizard` | Configure 1Password as the secret backend |
| `penv export` | Export preset(s) to a folder, optionally zip |
| `penv op pull <preset> …` | Pull one or more presets from 1Password |
| `penv op push <preset> …` | Push one or more presets to 1Password |
| `penv op new-preset` | Interactively copy a preset to a new name |
| `penv op diff <preset>` | Compare local .env files vs 1Password |
| `penv op list` | List all presets in 1Password |
| `penv morning-check` | Start-of-day sync check |
| `penv load-env <preset>` | Load a preset into service repos |
| `penv dev-ui` | Launch a configured tmux UI session |
| `penv dev-backend` | Launch a configured tmux backend session |
| `penv dev-session` | Launch a flexible grid session defined in `sessions[]` |

## Documentation

- [Setup guide](docs/setup.md) — install, first-time configuration, daily workflow
- [Configuration reference](docs/configuration.md) — `settings.json` fields, file layout, path resolution
- [Extending penv](docs/extending.md) — how to wrap penv as a library to add project-specific backends and commands

## License

MIT
