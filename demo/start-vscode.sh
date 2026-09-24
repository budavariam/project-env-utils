#!/usr/bin/env bash
# Open the VS Code workspace. With no args: interactive guide.
# With args: passed directly to 'workspace open'.
# Examples:
#   ./start-vscode.sh                          # interactive guide
#   ./start-vscode.sh --branch feat_my-branch  # all repos in a worktree
#   ./start-vscode.sh --group ui=feat_frontend # UI repo in worktree only
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ $# -eq 0 ]; then
    exec "$SCRIPT_DIR/start.sh" workspace guide
else
    exec "$SCRIPT_DIR/start.sh" workspace open "$@"
fi
