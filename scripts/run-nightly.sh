#!/usr/bin/env bash
# Build Soufflé Nightly (debug identity) and open it. Does not touch the
# installed Soufflé.app.
#   --dmg    wrap the .app in a disk image
#   --fresh  wipe Nightly data + TCC rows, and remove the old debug Soufflé.app
#            bundle that collides in System Settings
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

app="$root/app/target/debug/bundle/macos/Soufflé Nightly.app"
old_debug="$root/app/target/debug/bundle/macos/Soufflé.app"
dmg="$root/app/target/debug/bundle/dmg/Soufflé Nightly.dmg"
nightly_id="com.souffle.desktop.nightly"

fresh=0
dmg_mode=0
for arg in "$@"; do
  case "$arg" in
    --fresh) fresh=1 ;;
    --dmg) dmg_mode=1 ;;
  esac
done

# Match on the ASCII tail only: Launch Services starts the binary with a
# decomposed "é" (e + U+0301) in its path, so a pattern typed with the
# precomposed "é" never matches and the old Nightly survives - `open` then just
# brings the stale build forward. "Nightly.app/" never matches
# /Applications/Soufflé.app.
nightly_proc="Nightly.app/Contents/MacOS/souffle"

# Stop every running Nightly and wait until it has exited, so `open` launches
# the fresh bundle instead of re-activating a dying instance.
stop_nightly() {
  pkill -f "$nightly_proc" 2>/dev/null || true
  local i
  for i in {1..50}; do
    pgrep -f "$nightly_proc" >/dev/null || return 0
    sleep 0.1
  done
  pkill -9 -f "$nightly_proc" 2>/dev/null || true
  for i in {1..20}; do
    pgrep -f "$nightly_proc" >/dev/null || return 0
    sleep 0.1
  done
  echo "error: Soufflé Nightly is still running after SIGKILL" >&2
  return 1
}

# Always stop Nightly before a rebuild. Never touch /Applications/Soufflé.
stop_nightly
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
  # Failures are printed, not swallowed: a service name macOS no longer
  # accepts is exactly what makes a --fresh run lie about its starting state.
  for svc in Accessibility AudioCapture Calendar Microphone ScreenCapture; do
    tccutil reset "$svc" "$nightly_id" >/dev/null || echo "  tccutil reset $svc failed (see above)"
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
# (bundle-macos.sh applies the same default and the same fallback-to-unsigned
# behavior if the identity isn't in the keychain.)

bundle_args=(--debug --nightly)
[[ "$dmg_mode" -eq 1 ]] && bundle_args+=(--dmg)
"$root/scripts/bundle-macos.sh" "${bundle_args[@]}"

stop_nightly

if [[ "$dmg_mode" -eq 1 ]]; then
  open "$dmg"
else
  open "$app"
fi

echo "Soufflé Nightly: $app"
