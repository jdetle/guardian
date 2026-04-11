#!/bin/bash
# Snooze Guardian prompt gates or allow the next submit once (human-in-the-loop).
# Usage:
#   bash scripts/guardian-resume.sh snooze [minutes]   # default 15
#   bash scripts/guardian-resume.sh proceed-once
#   bash scripts/guardian-resume.sh clear-snooze
set -euo pipefail

GUARDIAN_DIR="${GUARDIAN_DIR:-$HOME/.guardian}"
mkdir -p "$GUARDIAN_DIR"

cmd="${1:-}"
case "$cmd" in
    snooze)
        min="${2:-15}"
        ts=""
        if ts=$(date -u -v+"${min}"M '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null); then
            :
        elif ts=$(date -u -d "+${min} minutes" '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null); then
            :
        else
            echo "Could not compute snooze time (need GNU or BSD date)." >&2
            exit 1
        fi
        printf '%s\n' "$ts" > "$GUARDIAN_DIR/snooze_until"
        echo "Guardian snoozed until $ts (UTC)"
        ;;
    proceed-once)
        touch "$GUARDIAN_DIR/proceed_once"
        echo "Created $GUARDIAN_DIR/proceed_once — your next gated submit will consume this."
        ;;
    clear-snooze)
        rm -f "$GUARDIAN_DIR/snooze_until"
        echo "Cleared snooze."
        ;;
    *)
        echo "Usage: $0 snooze [minutes] | proceed-once | clear-snooze" >&2
        exit 1
        ;;
esac
