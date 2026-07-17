# CLAUDE.md

Rust library and standalone binary (`penv`) for preset-based `.env` management.
Lives at `~/project/project-env-utils/`.

## Structure

| Path | Purpose |
|---|---|
| `src/lib.rs` | Public API — all backends, commands, config types |
| `src/main.rs` | Standalone `penv` CLI (thin wrapper around the lib) |
| `src/cmd/` | One module per command |
| `src/backend/` | Secret backend implementations |
| `settings.example.json` | Reference config for new projects |
| `settings.schema.json` | JSON Schema for `settings.json` validation |
| `docs/` | Setup guide, configuration reference, extending guide |

## Building

```bash
cargo build --release
```

Tests:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## After pushing changes — bump the submodule in the project wrapper

The project wrapper embeds this repo as a git submodule pinned to a specific commit.
**After every push here, update the pointer in the wrapper:**

```bash
cd ~/project/your-wrapper
cd project-env-utils && git pull origin main && cd ..
git add project-env-utils
git commit -m "chore: bump project-env-utils"
git push
```

Skipping this means the wrapper keeps building against the old commit until the pointer is bumped.

## Adding a new command

1. Create `src/cmd/<name>.rs` with a `pub fn run(...)` entry point
2. Register it in `src/cmd/mod.rs`
3. Add the CLI variant in `src/main.rs` under `Command` and the match arm
4. If the command needs a shell helper function in dev sessions, add it to `write_close_session_script` and `write_teardown_script` in `src/cmd/worktree.rs`
5. If it should appear in the session info box, add it to `SessionNotes` in the relevant `dev_*.rs` commands
