#!/usr/bin/env bash
# record-demo.sh — Record an asciinema demo from a screenplay script.
#
# Usage:
#   ./scripts/record-demo.sh <screenplay.sh> [output.cast]
#
# The screenplay is a plain shell script that calls these functions:
#
#   run "command"        — type the command character-by-character, then press Enter
#   type_text "text"     — type text character-by-character (no Enter)
#   enter                — press Enter
#   ctrl_c               — send Ctrl-C
#   pause [seconds]      — wait (default 1s)
#   clear_screen         — send 'clear' + Enter
#   blank_lines [n]      — print n blank lines (visual breathing room, default 1)
#
# Example screenplay (scripts/demo-screenplay.sh):
#   run "penv load-env dev"
#   pause 2
#   run "penv env-age --service -api"
#
# Requirements: tmux, asciinema
#
set -euo pipefail

SCREENPLAY="${1:?Usage: $0 <screenplay.sh> [output.cast]}"
OUTPUT="${2:-demo.cast}"

# ── Config ────────────────────────────────────────────────────────────────────

COLS=120
ROWS=30
CHAR_DELAY=0.04   # seconds between keystrokes (human-like typing speed)
SESSION="penv-demo-$$"

# ── Screenplay API ─────────────────────────────────────────────────────────────

_TARGET=""

# Type text one character at a time into the recording pane.
type_text() {
    local text="$1"
    local i
    for ((i = 0; i < ${#text}; i++)); do
        local ch="${text:$i:1}"
        tmux send-keys -t "$_TARGET" -l "$ch"
        sleep "$CHAR_DELAY"
    done
}

# Type a full command and press Enter.
run() {
    type_text "$1"
    sleep 0.3
    tmux send-keys -t "$_TARGET" Enter
}

# Press Enter on its own.
enter() {
    tmux send-keys -t "$_TARGET" Enter
}

# Send Ctrl-C.
ctrl_c() {
    tmux send-keys -t "$_TARGET" C-c
    sleep 0.3
}

# Wait before the next action.
pause() {
    sleep "${1:-1}"
}

# Clear the screen.
clear_screen() {
    tmux send-keys -t "$_TARGET" "clear" Enter
    sleep 0.5
}

# Print blank lines for visual breathing room.
blank_lines() {
    local n="${1:-1}"
    local i
    for ((i = 0; i < n; i++)); do
        tmux send-keys -t "$_TARGET" Enter
    done
    sleep 0.2
}

export -f type_text run enter ctrl_c pause clear_screen blank_lines

# ── PS1 for the recording shell ───────────────────────────────────────────────

RCFILE=$(mktemp /tmp/demo-rc-XXXXXX.sh)
trap 'rm -f "$RCFILE"; tmux kill-session -t "$SESSION" 2>/dev/null || true' EXIT

cat > "$RCFILE" << 'RCEOF'
# Minimal rc for demo recording — clean PS1, no history, no prompts
export HISTFILE=/dev/null
export HISTSIZE=0

_git_branch() {
    git -C . rev-parse --abbrev-ref HEAD 2>/dev/null | sed 's/.*/ (&)/'
}

# Format: dirname (branch)$
export PS1='\[\e[0;32m\]\W\[\e[0;33m\]$(_git_branch)\[\e[0m\]$ '

# Bring in the user's PATH and usual env so the tools work
[ -f /etc/profile ] && source /etc/profile 2>/dev/null || true
[ -f ~/.profile ]   && source ~/.profile   2>/dev/null || true
[ -f ~/.bashrc ]    && source ~/.bashrc    2>/dev/null || true

# Override PS1 again after sourcing (it may have been reset above)
export PS1='\[\e[0;32m\]\W\[\e[0;33m\]$(_git_branch)\[\e[0m\]$ '
RCEOF

# ── Start recording ───────────────────────────────────────────────────────────

echo "Starting recording → $OUTPUT"
echo "  Screenplay: $SCREENPLAY"
echo "  Terminal:   ${COLS}x${ROWS}"
echo ""

# Create a detached tmux session at the required size.
tmux new-session -d -s "$SESSION" -x "$COLS" -y "$ROWS"
_TARGET="$SESSION:0.0"

# Launch asciinema inside the tmux pane.
tmux send-keys -t "$_TARGET" \
    "asciinema rec --overwrite --cols $COLS --rows $ROWS '$OUTPUT' --command 'bash --rcfile $RCFILE'" \
    Enter

# Give asciinema time to start and display its opening message.
sleep 2

# ── Run the screenplay ────────────────────────────────────────────────────────

source "$SCREENPLAY"

# Give the last command's output time to appear, then stop recording.
sleep 1
tmux send-keys -t "$_TARGET" "" "C-d"

# Wait for asciinema to finalise the file.
sleep 2

echo ""
echo "Saved: $OUTPUT"
