#!/bin/bash
# Claude Code UserPromptSubmit — prompt gate (blocks new prompts at critical load).
# Claude Code and Codex share one hook protocol; the implementation lives in ../codex/.
set -euo pipefail
export GUARDIAN_HOOK_FORMAT=claude
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
exec bash "$SCRIPT_DIR/codex/user-prompt-submit.sh"
