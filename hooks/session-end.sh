#!/bin/bash
# Guardian sessionEnd hook (Cursor): the conversation is over. Marks the session ended in
# sessions.db. Claude Code and Codex use codex/session-end.sh. Prints nothing.
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
exec bash "$SCRIPT_DIR/session-event.sh" end
