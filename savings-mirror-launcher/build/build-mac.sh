#!/bin/bash
# Build SavingsMirror.app for macOS and drop it on the Desktop.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ ! -f assets/icon.icns ]; then
  echo "FAIL: assets/icon.icns ontbreekt — geen icon, geen build."
  exit 1
fi

if ! command -v cargo-bundle >/dev/null 2>&1 \
   && ! cargo bundle --help >/dev/null 2>&1; then
  echo "FAIL: cargo-bundle not installed. Run: cargo install cargo-bundle"
  exit 1
fi

echo "→ cargo bundle --release"
cargo bundle --release

APP_PATH="target/release/bundle/osx/SavingsMirror.app"
if [ ! -d "$APP_PATH" ]; then
  echo "FAIL: expected bundle at $APP_PATH but it is missing"
  exit 1
fi

# cargo-bundle has no metadata key for LSUIElement, so patch Info.plist
# post-bundle. LSUIElement=true keeps the dock icon from flashing on launch.
plutil -replace LSUIElement -bool true "$APP_PATH/Contents/Info.plist" || true

DEST="$HOME/Desktop/SavingsMirror.app"
rm -rf "$DEST"
cp -R "$APP_PATH" "$DEST"

# Touch the bundle so Finder/LaunchServices refresh.
touch "$DEST"

echo ""
echo "OK → $DEST"
du -sh "$DEST"
