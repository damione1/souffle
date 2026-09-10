#!/usr/bin/env bash
# Build Soufflé Nightly (debug identity) and open it. Does not touch the
# installed Soufflé.app. Pass --dmg to wrap the .app in a disk image first.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

app="$root/src-tauri/target/debug/bundle/macos/Soufflé Nightly.app"
dmg="$root/src-tauri/target/debug/bundle/dmg/Soufflé Nightly.dmg"

npm run tauri -- build --debug --bundles app

# A rebuild must replace the running Nightly process. Leave /Applications/Soufflé alone.
pkill -f "Soufflé Nightly.app/Contents/MacOS/souffle" 2>/dev/null || true

if [[ "${1:-}" == "--dmg" ]]; then
  mkdir -p "$(dirname "$dmg")"
  stage="$(mktemp -d)"
  cp -R "$app" "$stage/"
  ln -s /Applications "$stage/Applications"
  hdiutil create -volname "Soufflé Nightly" -srcfolder "$stage" -ov -format UDZO "$dmg"
  rm -rf "$stage"
  open "$dmg"
else
  open "$app"
fi

echo "Soufflé Nightly: $app"
