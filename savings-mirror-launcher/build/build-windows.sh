#!/bin/bash
# Cross-compile SavingsMirror.exe for Windows and zip a distribution folder.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ ! -f assets/icon.ico ]; then
  echo "FAIL: assets/icon.ico ontbreekt — geen icon, geen build."
  exit 1
fi

if ! command -v cargo-xwin >/dev/null 2>&1 \
   && ! cargo xwin --help >/dev/null 2>&1; then
  echo "FAIL: cargo-xwin not installed. Run: cargo install cargo-xwin"
  exit 1
fi

if ! rustup target list --installed | grep -q "x86_64-pc-windows-msvc"; then
  echo "FAIL: rust target x86_64-pc-windows-msvc not installed."
  echo "      Run: rustup target add x86_64-pc-windows-msvc"
  exit 1
fi

echo "→ cargo xwin build --release --target x86_64-pc-windows-msvc"
cargo xwin build --release --target x86_64-pc-windows-msvc

EXE_PATH="target/x86_64-pc-windows-msvc/release/savings-mirror-launcher.exe"
if [ ! -f "$EXE_PATH" ]; then
  echo "FAIL: expected exe at $EXE_PATH but it is missing"
  exit 1
fi

PACK="target/windows-pack/SavingsMirror"
rm -rf target/windows-pack
mkdir -p "$PACK"
cp "$EXE_PATH" "$PACK/SavingsMirror.exe"
cp assets/icon.ico "$PACK/icon.ico"
cat > "$PACK/README.txt" <<'EOF'
SavingsMirror launcher for Windows
==================================

Double-click SavingsMirror.exe — a tray icon appears in the system tray
(bottom-right, next to the clock).

Right-click the tray icon to open the menu:
  Status: …          live status indicator
  Start runtime      spawns savings-mirror
  Stop runtime       terminates savings-mirror
  Open dashboard     opens http://127.0.0.1:8991 in your default browser
  Quit               stops the runtime and exits

Place savings-mirror.exe next to SavingsMirror.exe, or set the
SAVINGS_MIRROR_BINARY environment variable to its full path.

Logs: %LOCALAPPDATA%\SavingsMirror\logs\launcher.log
EOF

# Zip the pack — use the system `zip` since it's present on macOS by default.
ZIPNAME="SavingsMirror-windows.zip"
( cd target/windows-pack && rm -f "$ZIPNAME" && zip -r "$ZIPNAME" SavingsMirror >/dev/null )

DEST="$HOME/Desktop/SavingsMirror-windows.zip"
cp "target/windows-pack/$ZIPNAME" "$DEST"

echo ""
echo "OK → $DEST"
ls -lh "$DEST"
