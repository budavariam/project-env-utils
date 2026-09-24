#!/usr/bin/env bash
# Build penv (no-op when sources are up to date), then run any penv command.
# Falls back to an existing binary if the build fails.
# Usage: ./start.sh <subcommand> [args...]
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PENV_BIN="$REPO_ROOT/target/release/penv"

if ! command -v cargo &>/dev/null; then
    echo "ERROR: cargo not found. Install Rust from https://rustup.rs" >&2
    exit 1
fi

if cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml"; then
    : # build succeeded (or was already up to date)
elif [[ -x "$PENV_BIN" ]]; then
    echo "Warning: build failed — using existing binary" >&2
else
    echo "ERROR: build failed and no existing binary found." >&2
    exit 1
fi

# Export so tmux panes and helper scripts inherit the config path automatically.
export PENV_REPO_ROOT="$SCRIPT_DIR/penv"
exec "$PENV_BIN" "$@"
