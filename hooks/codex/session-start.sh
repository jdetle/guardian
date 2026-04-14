#!/bin/bash
# Codex SessionStart — same context as Cursor sessionStart; emits Codex hook JSON.
set -euo pipefail
export GUARDIAN_HOOK_FORMAT=codex
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
exec bash "$SCRIPT_DIR/session-start.sh"
