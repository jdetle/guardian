#!/bin/bash
# Codex / Claude Code PostToolUse — records the tool call and the load it ran under in sessions.db.
# Record only: prints nothing and exits 0, which in both agents means "nothing to say".
# (hooks/claude/post-tool-use.sh reuses this with GUARDIAN_HOOK_FORMAT=claude.)
set -uo pipefail
export GUARDIAN_HOOK_FORMAT="${GUARDIAN_HOOK_FORMAT:-codex}"
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
exec bash "$SCRIPT_DIR/session-event.sh" tool
