#!/usr/bin/env bash
# Build Soufflé Nightly (debug identity) and open it. Does not touch the
# installed Soufflé.app.
#   --dmg    wrap the .app in a disk image
#   --fresh  wipe Nightly data + TCC rows, and remove the old debug Soufflé.app
#            bundle that collides in System Settings
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

app="$root/src-tauri/target/debug/bundle/macos/Soufflé Nightly.app"
old_debug="$root/src-tauri/target/debug/bundle/macos/Soufflé.app"
dmg="$root/src-tauri/target/debug/bundle/dmg/Soufflé Nightly.dmg"
nightly_id="com.souffle.desktop.nightly"

fresh=0
dmg_mode=0
for arg in "$@"; do
  case "$arg" in
    --fresh) fresh=1 ;;
    --dmg) dmg_mode=1 ;;
  esac
done

# Always stop Nightly before a rebuild. Never touch /Applications/Soufflé.
pkill -f "Soufflé Nightly.app/Contents/MacOS/souffle" 2>/dev/null || true
# Same-name debug bundle from earlier builds: TCC shows "Soufflé" and
# Launch Services can pick it over Nightly.
if [[ -d "$old_debug" ]]; then
  echo "Removing stale debug bundle: $old_debug"
  rm -rf "$old_debug"
fi

if [[ "$fresh" -eq 1 ]]; then
  echo "Wiping Nightly data and TCC rows ($nightly_id)"
  rm -rf "${HOME}/Library/Application Support/${nightly_id}"
  rm -rf "${HOME}/Library/WebKit/${nightly_id}"
  for svc in Accessibility AudioCapture Calendar Microphone ScreenCapture; do
    tccutil reset "$svc" "$nightly_id" >/dev/null 2>&1 || true
  done
fi

# TCC keys its records on the designated requirement, not on the bundle id. An
# unsigned bundle's DR is its cdhash, so every rebuild is a new subject and all
# permissions have to be granted again. Signing with a stable Developer ID gives
# a DR of the form `identifier ... and certificate leaf[subject.OU] = X6H966RSDB`,
# which survives a rebuild. No notarization needed: a locally built bundle
# carries no quarantine flag, so Gatekeeper never asks.
#
# The first build after this fails with `errSecInternalComponent` unless the
# keychain prompt for the signing key is answered with "Always Allow": the
# private key's ACL does not list codesign. One click, once per machine.
export APPLE_SIGNING_IDENTITY="Developer ID Application: Damien Goehrig (X6H966RSDB)"

npm run tauri -- build --debug --bundles app

pkill -f "Soufflé Nightly.app/Contents/MacOS/souffle" 2>/dev/null || true

if [[ "$dmg_mode" -eq 1 ]]; then
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
