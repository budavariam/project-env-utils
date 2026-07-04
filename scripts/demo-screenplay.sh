#!/usr/bin/env bash
# demo-screenplay.sh — Example screenplay for record-demo.sh.
#
# Run with:
#   ./scripts/record-demo.sh scripts/demo-screenplay.sh demo.cast
#
# Each function call maps to one action in the recording.
# Adjust pause lengths to match how long each command takes on your machine.

# ── Setup ─────────────────────────────────────────────────────────────────────

# Navigate to the tool directory (adjust to your actual path)
run "cd ~/project/project-env-utils"
pause 0.5
clear_screen

# ── Show current state ────────────────────────────────────────────────────────

run "cat settings.json | grep -A4 '\"project\"'"
pause 1.5

# ── Load a preset ─────────────────────────────────────────────────────────────

run "penv load-env dev"
pause 2

# ── Check env age across locations ────────────────────────────────────────────

run "penv env-age --service -api --preset dev"
pause 1
# Choose 's' to skip the action menu
type_text "s"
enter
pause 1

# ── Morning check (shows drift detection) ─────────────────────────────────────

run "penv morning-check"
pause 2
# Skip each entry
type_text "s"
enter
pause 0.5

# ── Switch to a different preset ──────────────────────────────────────────────

run "penv load-env test"
pause 2

# ── Done ──────────────────────────────────────────────────────────────────────

pause 1
