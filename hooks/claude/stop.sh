#!/bin/bash
# Claude Code Stop — the turn is over: records it and scores the session so far.
# Claude Code and Codex share one hook protocol; the implementation lives in ../codex/.
set -uo pipefail
export GUARDIAN_HOOK_FORMAT=claude
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
exec bash "$SCRIPT_DIR/codex/stop.sh"
