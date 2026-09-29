#!/usr/bin/env bash
# ui-shot.sh: boot a macOS .app bundle and capture a screenshot cropped to
# just its window, once it becomes frontmost.
#
# Requires two permissions granted to the calling terminal app (System
# Settings > Privacy & Security), each of which only takes effect after a
# full quit + relaunch of the terminal, not just a new window/tab:
#   - Screen Recording   (to capture pixels at all)
#   - Accessibility      (to read window position/size via System Events,
#                          for cropping to the window instead of full screen)
# Without Accessibility this script still works but falls back to a
# full-screen capture.
#
# Usage: ui-shot.sh <path-to-.app> [output.png] [settle-seconds]
set -euo pipefail

app="$1"
out="${2:-./screenshot.png}"
settle="${3:-2}"

if [[ ! -d "$app" ]]; then
  echo "error: not a bundle: $app" >&2
  exit 1
fi

name="$(basename "$app" .app)"

# Launch Services starts the binary with a decomposed (NFD) path, so a bundle
# name with an accent ("Soufflé Nightly") typed precomposed never matches:
# kill on both forms, then wait for the old instance to actually exit.
app_nfd="$(printf '%s' "$app" | iconv -f UTF-8 -t UTF-8-MAC)"
for pattern in "$app/Contents/MacOS/" "$app_nfd/Contents/MacOS/"; do
  pkill -f "$pattern" 2>/dev/null || true
done
for _ in $(seq 1 50); do
  pgrep -f "$app/Contents/MacOS/" >/dev/null || pgrep -f "$app_nfd/Contents/MacOS/" >/dev/null || break
  sleep 0.1
done

open -a "$app"

# The Accessibility process name is the bundle's executable name (e.g.
# "souffle"), NOT the bundle display name (e.g. "Soufflé Nightly") - the two
# differ for this project, so poll on frontmost-ness rather than assuming
# they match, and use whatever name actually shows up as frontmost.
frontmost=""
for _ in $(seq 1 30); do
  frontmost=$(osascript -e 'tell application "System Events" to get name of first application process whose frontmost is true' 2>/dev/null || echo "")
  [[ -n "$frontmost" ]] && [[ "$frontmost" != "loginwindow" ]] && break
  sleep 0.3
done

sleep "$settle"

mkdir -p "$(dirname "$out")"

query_name="${frontmost:-$name}"
bounds=$(osascript -e "tell application \"System Events\" to tell process \"$query_name\" to get {position, size} of front window" 2>/dev/null || echo "")

if [[ -n "$bounds" ]]; then
  # bounds looks like: "x, y, w, h"
  IFS=',' read -r x y w h <<<"${bounds// /}"
  screencapture -x -R "${x},${y},${w},${h}" "$out"
else
  echo "warning: could not read window bounds (Accessibility permission missing/stale?), falling back to full-screen capture" >&2
  screencapture -x "$out"
fi

if [[ "$frontmost" != "$name" ]]; then
  echo "note: frontmost process is '$frontmost' (bundle display name is '$name') - this is expected when they differ, verify the screenshot looks right" >&2
fi
echo "screenshot: $out"
