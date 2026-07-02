# Configuration reference

## File locations

All paths are relative to `PENV_REPO_ROOT` (the project root directory).

| Path | Tracked? | Purpose |
|---|---|---|
| `settings.json` | ✓ yes | Project config — services, vault, tmux sessions |
| `settings.local.json` | ✗ gitignore | Personal overrides — secret backend, vault name |
| `env/<service>/<preset>.env` | ✓ yes | Preset profiles — no secrets, used as fallback |
| `local/<service>.<preset>.env` | ✗ gitignore | Full preset cache pulled from backend |
| `local/<service>.env` | ✗ gitignore | Generic fallback when no preset matched |
| `backups/` | ✗ gitignore | Timestamped snapshots from morning-check |
| `.state/active.json` | ✗ gitignore | Which preset is active per service |

Service `.env` files are written to `../my-service/.env` (the service repo, one level up from the project root).

---

## settings.json

Full example with all supported fields:

```json
{
  "project": {
    "name": "MyProject",
    "op_vault": "MyProject Dev",
    "op_item_prefix": "myproj",
    "bucket": ""
  },
  "services": [
    {
      "name": "my-api",
      "description": "Backend API — shown as the 1Password item note",
      "env_vars": ["NODE_ENV", "POSTGRES_HOST", "REDIS_HOST"]
    },
    {
      "name": "my-ui",
      "description": "Frontend application",
      "env_vars": ["VITE_API_URL"]
    }
  ],
  "dev_ui": {
    "repo": "my-ui",
    "pane_dev_cmd": "npm install && npm run dev",
    "pane_sb_cmd": "npm run storybook"
  },
  "dev_backend": {
    "session_name": "myproject-backend",
    "window_backend": [
      { "repo": "my-api", "cmd": "npm install && npm run dev" }
    ],
    "window_service": [
      { "repo": "my-other-service", "cmd": "python -m uvicorn app:app --reload" }
    ]
  },
  "claude": {
    "enabled": false,
    "start_dir": "~/projects/myproject"
  }
}
```

### `project` fields

| Field | Description |
|---|---|
| `name` | Human-readable project name shown in logs |
| `op_vault` | 1Password vault name (created by `penv op init-vault`) |
| `op_item_prefix` | Prefix for all vault item titles. `"myproj"` → items named `myproj/my-api`. Leave empty for bare names. |
| `bucket` | Reserved for future backends |

### `services[]` fields

| Field | Description |
|---|---|
| `name` | Must match the service's repo folder name |
| `description` | Stored as the item note in 1Password |
| `env_vars` | Key env var names shown in `penv show-info` summaries |

### `dev_ui` fields

Config for `penv dev-ui`. Runs two tmux panes in a single window.

| Field | Description |
|---|---|
| `repo` | Repo folder name (also used as worktree service name) |
| `pane_dev_cmd` | Command for the dev server pane |
| `pane_sb_cmd` | Command for the Storybook / secondary pane |

### `dev_backend` fields

Config for `penv dev-backend`. Opens one tmux window per pane group.

| Field | Description |
|---|---|
| `session_name` | tmux session name |
| `window_backend[]` | Repos run in the main backend window (each gets a pane) |
| `window_service[]` | Repos run in the services window (supporting processes) |

---

## sessions[]

Flexible alternative to `dev_ui` / `dev_backend`.  Used by `penv dev-session`.

Each entry describes one named tmux session with any number of tabs (tmux windows), each laid out in an **at most 2-column grid** with unlimited rows.  Grid positions not covered by a configured pane become idle shells automatically.

```json
{
  "sessions": [
    {
      "session_name": "myproject-dev",
      "tabs": [
        {
          "name": "backend",
          "panes": [
            { "repo": "my-api",  "cmd": "npm install && npm run dev", "col": 0, "row": 0 },
            { "repo": "my-ui",   "cmd": "npm install && npm run dev", "col": 0, "row": 1 },
            { "repo": "my-ui",   "cmd": "npm run storybook",          "col": 1, "row": 1 }
          ]
        },
        {
          "name": "services",
          "panes": [
            { "repo": "my-other", "cmd": "python -m uvicorn app:app --reload", "col": 0, "row": 0 }
          ]
        }
      ]
    }
  ]
}
```

The example above produces the following layout in the **backend** window:

```
┌──────────────────┬──────────────────┐
│ my-api dev  r0c0 │  (idle shell)    │  ← col 1 row 0 — not configured → empty shell
├──────────────────┤  r0c1            │
│ my-ui dev   r1c0 ├──────────────────┤
│                  │ my-ui storybook  │  ← r1c1
└──────────────────┴──────────────────┘
```

The idle shell at `(col=1, row=0)` receives the `show-info` / `close_session` / `reload_env` helper script.

### `sessions[]` fields

| Field | Description |
|---|---|
| `session_name` | tmux session name |
| `tabs[]` | Ordered list of tmux windows |

### `tabs[]` fields

| Field | Description |
|---|---|
| `name` | Window name shown in the tmux status bar |
| `panes[]` | Pane definitions placed into the grid |

### `panes[]` fields

| Field | Default | Description |
|---|---|---|
| `col` | `0` | Column index — `0` (left) or `1` (right). Values above 1 are clamped to 1. |
| `row` | `0` | Row index — `0` is the top row. |
| `repo` | `""` | Repo folder name (relative to `repo_parent()`). Empty = working dir is `repo_parent()`. |
| `cmd` | `""` | Shell command to run. Empty = idle shell (no command sent). |

**Gap rule:** if `n_cols` or `n_rows` in the grid is larger than the number of configured panes, every uncovered `(row, col)` position is created as an idle shell.  The first such position found (row-major order) receives the session helper script.

**Command:** `penv dev-session [--session <name>] [--preset <name>] [--attach]`

| Flag | Description |
|---|---|
| `--session <name>` | Required when more than one session is in `sessions[]` |
| `--preset <name>` | Override the active preset; omit to prompt |
| `--attach` | Reattach to an existing session instead of creating a new one |

---

## settings.local.json

Personal overrides — never committed. Created by `penv setup-wizard` or written manually.

```json
{
  "secret_backend": "onepassword",
  "onepassword_vault": "My Personal Vault",
}
```

| Field | Values | Description |
|---|---|---|
| `secret_backend` | `"onepassword"` \| `"<custom>"` \| (absent) | Which backend to use. Absent = local-only. See [extending penv](extending.md). |
| `onepassword_vault` | string | Overrides `project.op_vault` for this machine |

---

## 1Password item layout

Each service is stored as one Secure Note item, with one section per preset:

```
Vault: MyProject Dev
  Item: myproj/my-api
    Section: test   →  field "env" = full .env content
    Section: uat    →  field "env" = full .env content
  Item: myproj/my-ui
    Section: test   →  field "env" = full .env content
```

The item title is `{op_item_prefix}/{service}` (or just `{service}` if no prefix is set). The item note is the service `description` from `settings.json`.

---

## Preset resolution order

When `penv load-env <preset>` runs for a service, it tries sources in this order and stops at the first hit:

1. **Secret backend** (1Password or extension-provided) — fetches `(service, preset)`
2. **`local/<service>.<preset>.env`** — preset-specific local file
3. **`local/<service>.env`** — generic local fallback
4. **`env/<service>/<preset>.env`** — tracked profile (no secrets)

If none match, the service is skipped with a warning.

---

## PENV_REPO_ROOT

Controls where penv looks for `settings.json`, `env/`, `local/`, and other project files.

| Scenario | How it's set |
|---|---|
| Global install (`/usr/local/bin/penv`) | Set `PENV_REPO_ROOT=/path/to/config-dir` in your shell |
| Project-local binary | Binary lives inside the config dir — inferred automatically |
| Tests / CI | Set `PENV_REPO_ROOT` to a temp directory |
