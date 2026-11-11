#!/bin/bash
# Codex SessionStart — same context as Cursor sessionStart; emits Codex hook JSON.
# (hooks/claude/session-start.sh reuses this with GUARDIAN_HOOK_FORMAT=claude.)
set -euo pipefail
export GUARDIAN_HOOK_FORMAT="${GUARDIAN_HOOK_FORMAT:-codex}"
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
exec bash "$SCRIPT_DIR/session-start.sh"
