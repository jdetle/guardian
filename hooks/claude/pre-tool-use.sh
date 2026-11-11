#!/bin/bash
# Claude Code PreToolUse — load advisory; optional subagent deny (never auto-allows).
# Claude Code and Codex share one hook protocol; the implementation lives in ../codex/.
set -euo pipefail
export GUARDIAN_HOOK_FORMAT=claude
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
exec bash "$SCRIPT_DIR/codex/pre-tool-use.sh"
