#!/bin/bash
# Codex / Claude Code Stop — the turn is over: records it and scores the session so far.
# Record only: prints nothing and exits 0, which in both agents means "nothing to say".
# (hooks/claude/stop.sh reuses this with GUARDIAN_HOOK_FORMAT=claude.)
set -uo pipefail
export GUARDIAN_HOOK_FORMAT="${GUARDIAN_HOOK_FORMAT:-codex}"
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
exec bash "$SCRIPT_DIR/stop.sh"
