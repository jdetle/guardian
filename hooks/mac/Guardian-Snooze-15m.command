#!/bin/bash
# Double-click in Finder (macOS) to snooze Guardian prompt gates for 15 minutes.
# Same as: ~/.guardian/guardian snooze 15
set -euo pipefail
exec "${HOME}/.guardian/guardian" snooze 15
