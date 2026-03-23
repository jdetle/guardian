#!/bin/bash
# Check if the current machine has been validated by guardian chaos tests.
# Returns 0 if validated, 1 if not.
#
# Usage:
#   bash tests/stress/check-validated.sh          # check if validated
#   bash tests/stress/check-validated.sh --json   # output full validation record
set -uo pipefail

VALIDATION_FILE="$HOME/.guardian/validations.json"

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
NC='\033[0m'

# Compute this machine's key
model=$(sysctl -n hw.model 2>/dev/null || echo "unknown")
build=$(sw_vers -buildVersion 2>/dev/null || echo "unknown")
arch=$(uname -m)
mem_gb=$(( $(sysctl -n hw.memsize 2>/dev/null || echo 0) / 1073741824 ))
key="${model}_${build}_${arch}_${mem_gb}gb"

echo -e "${CYAN}Machine: $key${NC}"
echo "  Chip: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown)"
echo "  OS: $(sw_vers -productVersion 2>/dev/null || echo unknown) ($build)"
echo "  Arch: $arch"
echo "  RAM: ${mem_gb}GB"
echo "  Docker: $(docker version --format '{{.Server.Arch}}' 2>/dev/null || echo 'not running')"

if [ ! -f "$VALIDATION_FILE" ]; then
    echo -e "\n${RED}NOT VALIDATED${NC}: No validation records found."
    echo "Run: bash tests/stress/chaos-escalate.sh"
    exit 1
fi

record=$(jq -r --arg key "$key" '.[] | select(.key == $key)' < "$VALIDATION_FILE" 2>/dev/null)

if [ -z "$record" ]; then
    echo -e "\n${RED}NOT VALIDATED${NC}: This machine/OS combination has never been tested."
    echo "Run: bash tests/stress/chaos-escalate.sh"
    exit 1
fi

if [ "${1:-}" = "--json" ]; then
    echo "$record" | jq .
    exit 0
fi

count=$(echo "$record" | jq -r '.validation_count // 0')
last=$(echo "$record" | jq -r '.last_validated // "never"')
chaos=$(echo "$record" | jq -r '.chaos_level_passed // 0')
chaos_tested=$(echo "$record" | jq -r '.chaos_tested // false')
version=$(echo "$record" | jq -r '.daemon_version // "unknown"')

echo ""
echo -e "${GREEN}VALIDATED${NC}"
echo "  Validation count: $count"
echo "  Last validated: $last"
echo "  Daemon version: $version"

if [ "$chaos_tested" = "true" ]; then
    echo -e "  Chaos tested: ${GREEN}yes${NC} (level $chaos/7)"
else
    echo -e "  Chaos tested: ${RED}no${NC}"
    echo "  Run: bash tests/stress/chaos-escalate.sh"
    exit 1
fi

if [ "$chaos" -ge 5 ]; then
    echo -e "  Status: ${GREEN}Chaos-hardened (level $chaos)${NC}"
    exit 0
elif [ "$chaos" -ge 3 ]; then
    echo -e "  Status: ${CYAN}Partially validated (level $chaos)${NC}"
    exit 0
else
    echo -e "  Status: ${RED}Minimally tested (level $chaos)${NC}"
    exit 1
fi
