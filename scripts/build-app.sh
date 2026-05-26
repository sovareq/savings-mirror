#!/usr/bin/env bash
# Build savings-mirror runtime + SavingsMirror.app launcher, drop .app on Desktop.
#
# Two-stage build:
#   1. Cargo-build the `savings-mirror` HTTP runtime (workspace root).
#   2. Cargo-build the menubar launcher and bundle it into a .app.
#
# The launcher resolves the runtime binary via `resolve_runtime_binary()` —
# it tries adjacent-to-launcher first, then the dev-path
# `~/savings-mirror/target/release/savings-mirror`. So a dev rebuild here is
# enough; for distribution copy the runtime binary into the .app bundle.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "→ build runtime"
cargo build --release --bin savings-mirror

echo "→ build launcher .app"
(
  cd savings-mirror-launcher
  bash build/build-mac.sh
)

DEST="$HOME/Desktop/SavingsMirror.app"
RUNTIME="$ROOT/target/release/savings-mirror"

# For distribution: copy runtime into the .app Resources so the launcher does
# not need a dev-path. Comment this out for in-tree development.
if [ -d "$DEST" ] && [ -f "$RUNTIME" ]; then
  cp "$RUNTIME" "$DEST/Contents/MacOS/savings-mirror"
  codesign --force --sign - "$DEST" >/dev/null 2>&1 || true
fi

echo ""
echo "OK → $DEST"
echo "Launch: open $DEST"
