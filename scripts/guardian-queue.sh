#!/bin/bash
# Guardian agent work queue — JSONL at ~/.guardian/agent_queue.jsonl
# Usage: guardian-queue add <title> <body> | list | peek | pop | count
set -euo pipefail

GUARDIAN_DIR="${GUARDIAN_DIR:-$HOME/.guardian}"
QUEUE_FILE="${GUARDIAN_QUEUE_FILE:-$GUARDIAN_DIR/agent_queue.jsonl}"
MAX_BODY_CHARS="${GUARDIAN_QUEUE_MAX_BODY:-200000}"

ensure_queue_file() {
    mkdir -p "$GUARDIAN_DIR"
    if [ ! -f "$QUEUE_FILE" ]; then
        : >"$QUEUE_FILE"
    fi
    chmod 600 "$QUEUE_FILE" 2>/dev/null || true
}

append_record() {
    local title="$1"
    local body="$2"
    local source="${3:-cli}"
    local conv="${4:-}"
    ensure_queue_file
    python3 - "$QUEUE_FILE" "$title" "$body" "$source" "$conv" "$MAX_BODY_CHARS" <<'PY'
import json, os, sys, uuid
from datetime import datetime, timezone

path, title, body, source, conv, max_c = sys.argv[1:8]
max_c = int(max_c)
body = (body or "")[:max_c]
rec = {
    "id": str(uuid.uuid4()),
    "enqueued_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "title": title,
    "body": body,
    "source": source,
}
if conv:
    rec["conversation_id"] = conv
line = json.dumps(rec, ensure_ascii=False) + "\n"
fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
try:
    os.write(fd, line.encode("utf-8"))
finally:
    os.close(fd)
PY
}

cmd="${1:-}"
case "$cmd" in
    add)
        if [ $# -lt 3 ]; then
            echo "usage: guardian-queue add <title> <body>" >&2
            exit 1
        fi
        title="$2"
        shift 2
        body="$*"
        append_record "$title" "$body" "cli" ""
        echo "Queued."
        ;;
    list)
        ensure_queue_file
        [ ! -s "$QUEUE_FILE" ] && echo "(empty)" && exit 0
        nl=1
        while IFS= read -r line; do
            [ -z "$line" ] && continue
            echo "--- #$nl ---"
            echo "$line" | jq .
            nl=$((nl + 1))
        done <"$QUEUE_FILE"
        ;;
    peek)
        ensure_queue_file
        head -1 "$QUEUE_FILE" | jq . 2>/dev/null || echo "(empty)"
        ;;
    pop)
        ensure_queue_file
        [ ! -s "$QUEUE_FILE" ] && echo "(empty)" && exit 0
        tmp=$(mktemp)
        tail -n +2 "$QUEUE_FILE" >"$tmp"
        mv "$tmp" "$QUEUE_FILE"
        chmod 600 "$QUEUE_FILE"
        echo "Removed first entry."
        ;;
    count)
        ensure_queue_file
        n=$(grep -c '^{' "$QUEUE_FILE" 2>/dev/null || echo 0)
        echo "$n"
        ;;
    enqueue-blocked-json)
        # Read Cursor beforeSubmitPrompt JSON from stdin; append one queue row if body extractable.
        ensure_queue_file
        python3 - "$QUEUE_FILE" "${MAX_BODY_CHARS}" <<'PY'
import json, os, sys, uuid
from datetime import datetime, timezone

path, max_c = sys.argv[1], int(sys.argv[2])
raw = sys.stdin.read()
try:
    data = json.loads(raw)
except json.JSONDecodeError:
    sys.exit(1)
body = (
    data.get("prompt")
    or data.get("promptText")
    or data.get("text")
    or data.get("message")
    or data.get("submission")
    or data.get("contents")
    or ""
)
if isinstance(body, dict):
    body = json.dumps(body)
if not isinstance(body, str):
    body = str(body)
body = body.strip()
if not body:
    sys.exit(2)
body = body[:max_c]
title = body.split("\n", 1)[0].strip()
if len(title) > 120:
    title = title[:117] + "..."
conv = data.get("conversation_id") or ""
roots = data.get("workspace_roots") or []
workspace_path = None
if isinstance(roots, list) and len(roots) > 0:
    w0 = roots[0]
    if isinstance(w0, str) and w0.strip():
        workspace_path = w0.strip()
rec = {
    "id": str(uuid.uuid4()),
    "enqueued_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "title": title,
    "body": body,
    "source": "blocked_submit",
}
if conv:
    rec["conversation_id"] = conv
if workspace_path:
    rec["workspace_path"] = workspace_path
line = json.dumps(rec, ensure_ascii=False) + "\n"
fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
try:
    os.write(fd, line.encode("utf-8"))
finally:
    os.close(fd)
print(rec["id"])
PY
        ;;
    *)
        echo "usage: guardian-queue add <title> <body> | list | peek | pop | count | enqueue-blocked-json <stdin-json>" >&2
        exit 1
        ;;
esac
