#!/bin/bash
# Render a Guardian hook template and merge it into an agent's JSON config
# (Claude Code settings.json, Codex hooks.json). Used by the installers; tested in the
# containerized suite.
#
# Usage: merge-agent-hooks.sh <target.json> <template.json> <placeholder> <hook-root> <backup-path>
#
# Guarantees:
#   - never writes a target it could not parse or merge (the file is left exactly as it was)
#   - re-running with nothing to change is a no-op: no write, and the backup is not overwritten
#   - the hook root may contain spaces, &, | or quotes (no sed; paths are shell-quoted on need)
#   - the new content is written to a temp file first, then moved into place
set -euo pipefail

if [ "$#" -ne 5 ]; then
    echo "usage: $0 <target.json> <template.json> <placeholder> <hook-root> <backup-path>" >&2
    exit 2
fi

TARGET="$1"
TEMPLATE="$2"
PLACEHOLDER="$3"
HOOK_ROOT="$4"
BACKUP="$5"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MARKER="/hooks/guardian/"

if ! command -v jq &>/dev/null; then
    echo "ERROR: jq is required to merge $TARGET safely." >&2
    exit 1
fi

TARGET_DIR="$(dirname "$TARGET")"
mkdir -p "$TARGET_DIR"
TMP_G="$(mktemp)"
TMP_OUT="$(mktemp "$TARGET_DIR/.guardian-merge.XXXXXX")"
trap 'rm -f "$TMP_G" "$TMP_OUT"' EXIT

# Literal substitution (split/join, not a regex), then quote the command for the shell only
# when the path needs it, so ordinary installs keep a plain readable command.
jq --arg ph "$PLACEHOLDER" --arg root "$HOOK_ROOT" '
    def shq: if test("^[A-Za-z0-9_/.@%+=:,-]+$") then . else @sh end;
    .hooks |= map_values(map(.hooks |= map(.command |= ((split($ph) | join($root)) | shq))))
' "$TEMPLATE" > "$TMP_G"

install_output() {
    # Keep a symlinked config (dotfile managers) a symlink: write through it.
    if [ -L "$TARGET" ]; then
        cat "$TMP_OUT" > "$TARGET"
    else
        chmod 644 "$TMP_OUT" 2>/dev/null || true
        if [ -f "$TARGET" ]; then
            # GNU form first: on Linux `stat -f` means "filesystem" and would print garbage.
            chmod "$(stat -c %a "$TARGET" 2>/dev/null || stat -f %Lp "$TARGET" 2>/dev/null || echo 644)" "$TMP_OUT" 2>/dev/null || true
        fi
        mv -f "$TMP_OUT" "$TARGET"
    fi
}

if [ ! -e "$TARGET" ]; then
    jq . "$TMP_G" > "$TMP_OUT"
    install_output
    echo "  Created $TARGET"
    exit 0
fi

if ! jq -e 'type == "object"' "$TARGET" >/dev/null 2>&1; then
    echo "ERROR: $TARGET is not a JSON object — not touching it. Fix it and re-run." >&2
    exit 1
fi

if ! merge_err=$(jq -s --arg marker "$MARKER" -f "$HERE/merge-agent-hooks.jq" "$TARGET" "$TMP_G" 2>&1 > "$TMP_OUT"); then
    echo "ERROR: cannot merge Guardian hooks into $TARGET — not touching it." >&2
    echo "  ${merge_err#jq: error (at *): }" >&2
    echo "  Fix that by hand (or move the file aside) and re-run." >&2
    exit 1
fi
jq empty "$TMP_OUT"

if [ "$(jq -S . "$TARGET")" = "$(jq -S . "$TMP_OUT")" ]; then
    echo "  Already up to date: $TARGET"
    exit 0
fi

cp "$TARGET" "$BACKUP"
install_output
echo "  Merged Guardian hooks into $TARGET (previous version: $BACKUP)"
