#!/bin/bash
# Guardian postToolUse hook (Cursor): records the tool call and the load it ran under in
# sessions.db. Claude Code and Codex use codex/post-tool-use.sh, which records the same way.
# Prints nothing; never blocks.
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
exec bash "$SCRIPT_DIR/session-event.sh" tool
