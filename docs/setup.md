# Setup guide

## Prerequisites

- Rust toolchain (`cargo build`)
- [1Password CLI](https://developer.1password.com/docs/cli/get-started/) (`op`) — run `op signin` before use
- tmux (optional — only needed for `dev-ui` / `dev-backend`)

---

## 1. Install the binary

```bash
git clone <this repo>
cd project-env-utils
cargo build --release
cp target/release/penv /usr/local/bin/penv
```

Or add to your shell profile so the binary always stays current:

```bash
alias penv='/path/to/project-env-utils/target/release/penv'
```

---

## 2. Create a project config directory

`penv` keeps all its project-specific files (settings, preset profiles, secret cache) in one directory called the **project root**. Tell penv where it is:

```bash
export PENV_REPO_ROOT=/path/to/your/config-dir
# Add to ~/.zshrc or ~/.bashrc to persist
```

If you install penv as a project-local binary (i.e. the `penv` binary lives inside the config dir), the variable is not needed — penv infers the directory from the binary's own location.

**Typical layout:**

```
my-project-env/          ← PENV_REPO_ROOT
  settings.json          ← project config (tracked in git)
  settings.local.json    ← personal overrides (gitignored)
  env/                   ← preset profiles without secrets (tracked)
    my-api/
      test.env
      uat.env
    my-ui/
      test.env
      uat.env
  local/                 ← credentials cache (gitignored)
    my-api.test.env
    my-api.uat.env
  backups/               ← morning-check snapshots (gitignored)
  .state/                ← active preset tracking (gitignored)
```

The service repos themselves live **alongside** the config dir (as siblings), not inside it:

```
projects/
  my-project-env/    ← PENV_REPO_ROOT
  my-api/            ← repo, .env written here by penv
  my-ui/             ← repo, .env written here by penv
```

---

## 3. Run the init wizard

```bash
penv init
```

This prompts for:

- **Project name** — displayed in logs
- **1Password vault** — where secrets are stored
- **Item prefix** — prepended to vault item titles (e.g. `myproj` → items named `myproj/my-api`)
- **Services** — repo names, descriptions, key env vars
- **Dev sessions** (optional) — tmux session config for `dev-ui` / `dev-backend`

It writes `settings.json` and optionally launches the 1Password setup wizard.

---

## 4. Configure the secret backend

```bash
penv setup-wizard
```

Walks through:

1. Checking `op` CLI is installed
2. Signing in (`op signin`)
3. Choosing a vault name
4. Writing `settings.local.json`
5. Running `penv op init-vault` to create the vault
6. Pulling or pushing your first preset

---

## 5. Seed 1Password with your first preset

If you already have `.env` files locally:

```bash
# Copy .env files into the local/ cache first
cp my-api/.env my-project-env/local/my-api.test.env
cp my-ui/.env  my-project-env/local/my-ui.test.env

# Push to 1Password
penv op push test
```

If 1Password already has secrets:

```bash
penv op pull test
```

---

## 6. Daily workflow

```bash
# Start of day — sync check and pull
penv morning-check

# Switch to a different preset for all services
penv op pull uat

# See what changed since last push
penv op diff test

# Create a personal dev preset from test
penv op new-preset    # interactive: copies test → my-dev, loads .env files

# Push local changes back
penv op push test
```

---

## Gitignore

Add these to your config dir's `.gitignore`:

```
local/
backups/
.state/
settings.local.json
```

Keep `env/` and `settings.json` tracked — they contain preset profiles and project config but no secrets.
