#!/bin/bash
# Render a Guardian hook template and merge it into an agent's JSON hook config
# (Cursor hooks.json, Claude Code settings.json, Codex hooks.json) — or remove Guardian's
# entries again. Used by the installers and scripts/uninstall.sh; tested in the containerized
# suite.
#
# Usage: merge-agent-hooks.sh <target.json> <template.json> <placeholder> <hook-root> <backup-path>
#        merge-agent-hooks.sh --remove <target.json> <hook-root> <backup-path>
#
# <hook-root> is where this agent's Guardian scripts live (e.g. ~/.claude/hooks/guardian).
# Only handlers whose command points into it are treated as Guardian's.
#
# Guarantees:
#   - never writes a target it could not parse or merge (the file is left exactly as it was)
#   - re-running with nothing to change is a no-op: no write, and the backup is not overwritten
#   - the hook root may contain spaces, &, | or quotes (no sed; paths are shell-quoted on need)
#   - the new content is written to a temp file first, then moved into place
#   - an existing backup is never overwritten (later ones get a timestamp suffix), so the
#     pre-Guardian file survives upgrades
#   - an empty (0-byte / whitespace) config is treated as "no config yet"
set -euo pipefail

MODE="merge"
if [ "${1:-}" = "--remove" ]; then
    MODE="remove"
    shift
fi

if { [ "$MODE" = "merge" ] && [ "$#" -ne 5 ]; } || { [ "$MODE" = "remove" ] && [ "$#" -ne 3 ]; }; then
    echo "usage: $0 <target.json> <template.json> <placeholder> <hook-root> <backup-path>" >&2
    echo "       $0 --remove <target.json> <hook-root> <backup-path>" >&2
    exit 2
fi

if [ "$MODE" = "merge" ]; then
    TARGET="$1"
    TEMPLATE="$2"
    PLACEHOLDER="$3"
    HOOK_ROOT="$4"
    BACKUP="$5"
else
    TARGET="$1"
    HOOK_ROOT="$2"
    BACKUP="$3"
fi
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK_ROOT="${HOOK_ROOT%/}"
if [ -z "$HOOK_ROOT" ]; then
    echo "ERROR: empty hook root — refusing to guess which hooks are Guardian's." >&2
    exit 2
fi

if ! command -v jq &>/dev/null; then
    echo "ERROR: jq is required to edit $TARGET safely." >&2
    exit 1
fi

# A config that exists but holds nothing (0 bytes or whitespace) is "no config yet".
target_has_content() {
    [ -e "$TARGET" ] && [ -n "$(tr -d '[:space:]' < "$TARGET" 2>/dev/null | head -c 1)" ]
}

if [ "$MODE" = "remove" ] && ! target_has_content; then
    exit 0
fi

TARGET_DIR="$(dirname "$TARGET")"
mkdir -p "$TARGET_DIR"
TMP_G="$(mktemp)"
TMP_OUT="$(mktemp "$TARGET_DIR/.guardian-merge.XXXXXX")"
trap 'rm -f "$TMP_G" "$TMP_OUT"' EXIT

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

if [ "$MODE" = "merge" ]; then
    # Literal substitution (split/join, not a regex) in every handler's command, then quote
    # it for the shell only when the path needs it, so ordinary installs keep a plain
    # readable command.
    jq --arg ph "$PLACEHOLDER" --arg root "$HOOK_ROOT" '
        def shq: if test("^[A-Za-z0-9_/.@%+=:,-]+$") then . else @sh end;
        walk(
            if type == "object" and ((.command? | type) == "string") and (.command | contains($ph)) then
                .command |= ((split($ph) | join($root)) | shq)
            else . end
        )
    ' "$TEMPLATE" > "$TMP_G"

    if ! target_has_content; then
        jq . "$TMP_G" > "$TMP_OUT"
        install_output
        echo "  Created $TARGET"
        exit 0
    fi
fi

# Exactly one JSON document, and it must be an object. (jq -s would silently keep only the
# first of several concatenated documents.)
if [ "$(jq -s 'length' "$TARGET" 2>/dev/null || echo bad)" != "1" ] \
    || ! jq -e 'type == "object"' "$TARGET" >/dev/null 2>&1; then
    echo "ERROR: $TARGET is not a single JSON object — not touching it. Fix it and re-run." >&2
    exit 1
fi

if [ "$MODE" = "merge" ]; then
    jq_inputs=("$TARGET" "$TMP_G")
else
    jq_inputs=("$TARGET")
fi
if ! merge_err=$(jq -s --arg root "$HOOK_ROOT" --arg mode "$MODE" -f "$HERE/merge-agent-hooks.jq" "${jq_inputs[@]}" 2>&1 > "$TMP_OUT"); then
    echo "ERROR: cannot $MODE Guardian hooks in $TARGET — not touching it." >&2
    echo "  ${merge_err#jq: error (at *): }" >&2
    echo "  Fix that by hand (or move the file aside) and re-run." >&2
    exit 1
fi
jq empty "$TMP_OUT"

if [ "$(jq -S . "$TARGET")" = "$(jq -S . "$TMP_OUT")" ]; then
    if [ "$MODE" = "merge" ]; then
        echo "  Already up to date: $TARGET"
    else
        echo "  No Guardian hooks in $TARGET"
    fi
    exit 0
fi

# Never overwrite an earlier backup: the first one is the pre-Guardian file.
if [ -e "$BACKUP" ]; then
    BACKUP="$BACKUP.$(date +%Y%m%d-%H%M%S)"
fi
cp "$TARGET" "$BACKUP"
install_output
if [ "$MODE" = "merge" ]; then
    echo "  Merged Guardian hooks into $TARGET (previous version: $BACKUP)"
else
    echo "  Removed Guardian hooks from $TARGET (previous version: $BACKUP)"
fi
