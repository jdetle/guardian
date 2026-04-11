#!/bin/bash
# Guardian subagentStart hook.
# ALWAYS allows. Provides system load context for agent awareness.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

input=$(read_hook_input)

pressure=$(read_state_pressure)
cpu=$(read_state_field "cpu_percent" "?")
mem=$(read_state_field "memory_available_gb" "?")
cursor_mb=$(read_state_field "cursor.resident_memory_megabytes" "0")
cursor_procs=$(read_state_field "cursor.process_count" "0")

disk_level=$(read_state_field "disk.level" "clear")
disk_used=$(read_state_field "disk.used_percent" "0")
disk_avail=$(read_state_field "disk.available_gb" "0")
disk_frag=""
if [ "$disk_level" = "warn" ] || [ "$disk_level" = "critical" ]; then
    disk_frag=" Disk: ~${disk_used}% used (~${disk_avail} GB free)."
fi

case "$pressure" in
    strained)
        json_output "$(jq -n \
            --arg msg "[Guardian] Monitoring active — moderate load (CPU: ${cpu}%, Mem free: ${mem}GB, Cursor ~${cursor_mb} MB RSS, ~${cursor_procs} Cursor processes).${disk_frag} Consider limiting parallel subagents. See hooks/resources.md for disk hygiene." \
            '{permission: "allow", agent_message: $msg}')"
        ;;
    critical)
        json_output "$(jq -n \
            --arg msg "[Guardian] Monitoring active — high load (CPU: ${cpu}%, Mem free: ${mem}GB, Cursor ~${cursor_mb} MB RSS, ~${cursor_procs} Cursor processes).${disk_frag} Prefer sequential work to reduce system pressure. See hooks/resources.md for disk hygiene." \
            '{permission: "allow", agent_message: $msg}')"
        ;;
    *)
        msg="[Guardian] Monitoring active — system nominal."
        if [ -n "$disk_frag" ]; then
            msg="${msg}${disk_frag} See hooks/resources.md for worktrees, Docker, caches."
        fi
        json_output "$(jq -n --arg msg "$msg" '{permission: "allow", agent_message: $msg}')"
        ;;
esac
