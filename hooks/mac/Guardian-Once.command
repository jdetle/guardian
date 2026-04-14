#!/bin/bash
# Double-click in Finder (macOS) to allow the next gated prompt submit once.
# Same as: ~/.guardian/guardian once
set -euo pipefail
exec "${HOME}/.guardian/guardian" once
