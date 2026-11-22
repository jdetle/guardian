#!/bin/bash
# Claude Code PostToolUse — records the tool call and the load it ran under in sessions.db.
# Claude Code and Codex share one hook protocol; the implementation lives in ../codex/.
set -uo pipefail
export GUARDIAN_HOOK_FORMAT=claude
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
exec bash "$SCRIPT_DIR/codex/post-tool-use.sh"
