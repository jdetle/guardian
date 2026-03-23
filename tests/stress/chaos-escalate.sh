#!/bin/bash
# Chaos Escalation Framework for Guardian Daemon — Containerized
#
# This script is a thin wrapper that launches tests inside Docker.
# It NEVER runs the daemon, stress tools, or chaos generators on the host.
#
# Usage: bash tests/stress/chaos-escalate.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

if [ "${TEST_MODE:-}" != "containerized" ]; then
    echo "=== Guardian Chaos Escalation (containerized) ==="
    echo "Building and running chaos tests inside Docker..."
    docker compose -f "$SCRIPT_DIR/docker-compose.test.yml" build guardian-test
    docker compose -f "$SCRIPT_DIR/docker-compose.test.yml" run --rm guardian-test
    exit $?
fi

echo "ERROR: This script should not be sourced inside the container."
echo "Use containerized-test.sh directly inside Docker."
exit 1
