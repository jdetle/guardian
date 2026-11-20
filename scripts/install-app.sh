#!/bin/bash
# Build the Guardian menu bar app and install it as ~/Applications/Guardian.app.
#
#   bash scripts/install-app.sh            # build, install, launch
#   bash scripts/install-app.sh --login    # ...and start it at every login
#   bash scripts/install-app.sh --no-login # ...and stop starting it at login
#
# The app is menu-bar only (no Dock icon). It reads ~/.guardian and runs the guardian CLI
# for snooze / allow-once / disk scans, so install the daemon first: bash scripts/install.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_NAME="Guardian.app"
BUNDLE_ID="com.guardian.menubar"
APPS_DIR="$HOME/Applications"
APP="$APPS_DIR/$APP_NAME"
LAUNCH_AGENTS="$HOME/Library/LaunchAgents"
PLIST="$LAUNCH_AGENTS/$BUNDLE_ID.plist"
DOMAIN="gui/$(id -u)"

LOGIN=""
for arg in "$@"; do
    case "$arg" in
        --login) LOGIN="on" ;;
        --no-login) LOGIN="off" ;;
        *) echo "usage: $0 [--login | --no-login]" >&2; exit 2 ;;
    esac
done

# Only ever replace our own app: something else may be installed under this name.
if [ -e "$APP" ]; then
    INSTALLED_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist" 2>/dev/null || true)"
    if [ "$INSTALLED_ID" != "$BUNDLE_ID" ]; then
        echo "ERROR: $APP exists and is not the Guardian menu bar app (its bundle id is not $BUNDLE_ID)." >&2
        echo "Move it aside, then run this again." >&2
        exit 1
    fi
fi

echo "=== Guardian menu bar app installer ==="

if [ ! -x "$HOME/.guardian/guardian" ]; then
    echo "Note: ~/.guardian/guardian is not installed yet. The app will show no data until you run: bash scripts/install.sh"
fi

if ! command -v swift >/dev/null 2>&1; then
    echo "ERROR: the Swift toolchain is needed to build the app. Install it with: xcode-select --install" >&2
    exit 1
fi

echo "[1/4] Building (swift build -c release)..."
(cd "$REPO_ROOT/app" && swift build -c release)
BUILT="$REPO_ROOT/app/.build/release/Guardian"
if [ ! -x "$BUILT" ]; then
    echo "ERROR: build produced no binary at $BUILT" >&2
    exit 1
fi

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -1)
VERSION="${VERSION:-0.0.0}"

echo "[2/4] Assembling $APP_NAME ($VERSION)..."
# Assemble next to the destination, then swap: a half-written bundle is never launched.
mkdir -p "$APPS_DIR"
STAGE="$(mktemp -d "$APPS_DIR/.guardian-app.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
NEW="$STAGE/$APP_NAME"
mkdir -p "$NEW/Contents/MacOS"
cp "$BUILT" "$NEW/Contents/MacOS/Guardian"
chmod +x "$NEW/Contents/MacOS/Guardian"
cat > "$NEW/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Guardian</string>
    <key>CFBundleDisplayName</key>
    <string>Guardian</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleExecutable</key>
    <string>Guardian</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.0</string>
    <!-- Menu bar only: no Dock icon, no app switcher entry. -->
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST
plutil -lint "$NEW/Contents/Info.plist" >/dev/null
# Ad-hoc signature: enough to run locally and to post notifications.
codesign --force --sign - "$NEW" >/dev/null 2>&1 || echo "  (codesign failed; the app still runs, but notifications may not)"

echo "[3/4] Installing to $APP..."
# Quit a running copy and wait for it to go, so the new one is the one in the menu bar.
if pgrep -qf "$APP/Contents/MacOS/Guardian"; then
    osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        pgrep -qf "$APP/Contents/MacOS/Guardian" || break
        sleep 0.5
    done
fi
launchctl bootout "$DOMAIN/$BUNDLE_ID" >/dev/null 2>&1 || true
if [ -e "$APP" ]; then
    rm -rf "$APPS_DIR/$APP_NAME"
fi
mv "$NEW" "$APP"

case "$LOGIN" in
    on)
        mkdir -p "$LAUNCH_AGENTS"
        cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$BUNDLE_ID</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/open</string>
        <string>-g</string>
        <string>$APP</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
</dict>
</plist>
PLIST
        plutil -lint "$PLIST" >/dev/null
        echo "  Starts at login: $PLIST"
        ;;
    off)
        rm -f "$PLIST"
        echo "  No longer starts at login."
        ;;
esac

echo "[4/4] Launching..."
open -g "$APP"

echo ""
echo "Guardian is in the menu bar (shield icon, top right)."
echo "  App:      $APP"
if [ -f "$PLIST" ]; then
    echo "  At login: yes (turn off: bash scripts/install-app.sh --no-login)"
else
    echo "  At login: no (turn on: bash scripts/install-app.sh --login)"
fi
echo "  Quit:     click the shield, then Quit"
