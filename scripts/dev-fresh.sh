#!/usr/bin/env bash
# Wipe Soufflé local state and start the dev app (first-run wizard).
set -euo pipefail

DATA_DIR="${HOME}/Library/Application Support/com.souffle.desktop.nightly"
WEBKIT_DIR="${HOME}/Library/WebKit/com.souffle.desktop.nightly"

echo "Removing app data:"
for dir in "$DATA_DIR" "$WEBKIT_DIR"; do
  if [[ -d "$dir" ]]; then
    echo "  $dir"
    rm -rf "$dir"
  fi
done

echo "Starting Tauri dev (first launch)…"
exec npm run tauri dev
