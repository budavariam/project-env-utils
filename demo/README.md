# project-env-utils demo

This directory is a self-contained example showing how to wire up a new project with `penv` using the **SQLite backend** — no external tools or accounts required.

## What this demo shows

- A two-service project (`demo-api`, `demo-ui`) with `dev` and `test` presets
- Using SQLite to store full `.env` secrets locally in a single file (`local/secrets.db`)
- The reference profiles in `env/` (non-secret vars, tracked in git)
- The recommended `settings.json` layout

## Quick start

### 1. Place the `penv` binary next to `settings.json`

Copy or symlink the `penv` binary into this directory:

```bash
cp /path/to/penv-binary ./penv
# or build from source:
cargo build --release --manifest-path ../Cargo.toml
cp ../target/release/penv ./penv
```

### 2. Create `settings.local.json`

```bash
cp settings.local.json.example settings.local.json
```

This selects the SQLite backend. The database is created automatically at `local/secrets.db` on first use.

### 3. Populate secrets

Push the reference profiles into the SQLite database as a starting point:

```bash
# First, copy reference profiles to service .env files (no secrets yet)
PENV_REPO_ROOT=$(pwd) ./penv load-env dev

# Or push existing .env files into the database:
# (place .env files in ../demo-api/.env and ../demo-ui/.env first)
PENV_REPO_ROOT=$(pwd) ./penv op push dev   # works with any backend incl. sqlite
```

Since there are no real service repos here, use `PENV_REPO_ROOT` to point penv at this directory and set up a test folder:

```bash
mkdir -p ../demo-api ../demo-ui

# Load from reference profiles (non-secret vars only)
PENV_REPO_ROOT=$(pwd) ./penv load-env dev
# → writes env/demo-api/dev.env → ../demo-api/.env
# → writes env/demo-ui/dev.env  → ../demo-ui/.env
```

### 4. Push secrets into SQLite

Once you have real `.env` files with secrets in `../demo-api/.env` and `../demo-ui/.env`:

```bash
PENV_REPO_ROOT=$(pwd) ./penv op push dev
# Reads ../demo-api/.env and ../demo-ui/.env → stores in local/secrets.db
```

### 5. Pull secrets back out

```bash
PENV_REPO_ROOT=$(pwd) ./penv op pull dev
# Reads from local/secrets.db → writes ../demo-api/.env and ../demo-ui/.env
```

### 6. List what's stored

```bash
PENV_REPO_ROOT=$(pwd) ./penv op list
```

### 7. Check age / sync status

```bash
PENV_REPO_ROOT=$(pwd) ./penv env-age --service demo-api --preset dev
```

## How the SQLite backend works

| File | Description |
|---|---|
| `settings.json` | Project config — service names, preset profiles, session layout |
| `settings.local.json` | Personal config — selects `sqlite` backend (gitignored) |
| `local/secrets.db` | SQLite database — stores all `(project, service, preset) → content` (gitignored) |
| `env/<service>/<preset>.env` | Reference profiles — non-secret vars, tracked in git |

The database schema:

```sql
CREATE TABLE env_secrets (
    project    TEXT NOT NULL,
    service    TEXT NOT NULL,
    preset     TEXT NOT NULL,
    content    TEXT NOT NULL,   -- full .env file content
    updated_at TEXT NOT NULL,   -- ISO 8601 UTC timestamp
    PRIMARY KEY (project, service, preset)
);
```

The `project` column maps to `settings.json` → `project.name`. Multiple projects can share the same `secrets.db` file without collisions.

## Directory layout

```
demo/
├── README.md                      ← this file
├── settings.json                  ← project config (tracked in git)
├── settings.local.json.example    ← copy to settings.local.json
├── env/
│   ├── demo-api/
│   │   ├── dev.env                ← reference profile (no secrets)
│   │   └── test.env
│   └── demo-ui/
│       ├── dev.env
│       └── test.env
└── local/                         ← gitignored (created on first use)
    ├── secrets.db                 ← SQLite database
    └── demo-project/              ← local cache folder (project-namespaced)
        ├── demo-api.dev.env
        └── demo-ui.dev.env
```

## Adding this to your own project

1. Copy `settings.json` and rename it to match your project.
2. Set `project.name` — this namespaces all cache files and DB rows.
3. Add your services to the `services` array.
4. Create reference profiles under `env/<service>/<preset>.env`.
5. Copy the `penv` binary to your project root.
6. Create `settings.local.json` with `{"secret_backend": "sqlite"}`.

That's it. Run `./penv load-env <preset>` to load env files into your service repos.
