# Changelog

All notable changes to project-env-utils are documented here.

## Unreleased

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
