#!/usr/bin/env bash
# demo-screenplay.sh — Example screenplay for record-asciinema.
#
# Run with:
#   record-asciinema scripts/demo-screenplay.sh scripts/demo.cast
#
# Each function call maps to one action in the recording.
# Adjust pause lengths to match how long each command takes on your machine.

# Tell record-asciinema where to start the recording shell.
# The penv config lives in demo/penv/ so that service repos (demo-api/, demo-ui/)
# are resolved as children of demo/ rather than siblings.
RECORDING_DIR=~/project/project-env-utils/demo

# ── Pre-recording: build penv and symlink into PATH (not shown) ───────────────

pause_recording

cargo build --release --manifest-path ~/project/project-env-utils/Cargo.toml --quiet
ln -sf ~/project/project-env-utils/target/release/penv ~/.local/bin/penv

continue_recording

# ── Setup ─────────────────────────────────────────────────────────────────────

clear_screen

run "export PENV_REPO_ROOT=~/project/project-env-utils/demo/penv"
pause 0.5

# ── Show current state ────────────────────────────────────────────────────────

run "pwd"

run "cat penv/settings.json | grep -A4 '\"project\"'"
pause 1.5

# ── Load a preset ─────────────────────────────────────────────────────────────

run "penv load-env dev"
pause 2

# ── Check env age across locations ────────────────────────────────────────────

run "penv env-age --service demo-api --preset dev"
pause 1
# Choose 's' to skip the action menu
type_text "s"
enter
pause 1

# ── Morning check (shows drift detection) ─────────────────────────────────────

run "penv morning-check"
pause 2
# Skip demo-api
type_text "s"
enter
pause 0.5
# Skip demo-ui
type_text "s"
enter
pause 0.5

# ── Switch to a different preset ──────────────────────────────────────────────

run "penv load-env test"
pause 2

# ── Launch UI dev session, watch it start, then detach ───────────────────────

run "penv dev-ui --preset dev"
pause 2      # startup sync-check prompt appears
enter        # skip sync (Enter = default skip)
pause 5      # session creates, panes start, recording shell attaches
tmux detach-client -s demo-ui 2>/dev/null || true
pause 1

# ── Launch backend dev session, watch it start, then detach ─────────────────

run "penv dev-backend --preset dev"
pause 2      # startup sync-check prompt
enter        # skip sync
pause 5      # session creates and attaches
tmux detach-client -s demo-backend 2>/dev/null || true
pause 1

# ── Done ──────────────────────────────────────────────────────────────────────

pause 1

# ── Post-recording cleanup: kill dev sessions (not shown in recording) ────────

pause_recording
tmux kill-session -t demo-ui      2>/dev/null || true
tmux kill-session -t demo-backend 2>/dev/null || true
