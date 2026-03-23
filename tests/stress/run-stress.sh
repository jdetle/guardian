#!/bin/bash
# Guardian stress tests — containerized runner.
# This script launches the Docker-based test environment.
# It NEVER runs the daemon or stress tools on the host.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

if [ "${TEST_MODE:-}" != "containerized" ]; then
    echo "=== Guardian Stress Tests (containerized) ==="
    echo "Building and running tests inside Docker..."
    docker compose -f "$SCRIPT_DIR/docker-compose.test.yml" build guardian-test
    docker compose -f "$SCRIPT_DIR/docker-compose.test.yml" run --rm guardian-test
    exit $?
fi

echo "ERROR: This script should not be sourced inside the container."
echo "Use containerized-test.sh directly inside Docker."
exit 1
