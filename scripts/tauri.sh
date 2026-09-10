#!/usr/bin/env bash
# Debug and `tauri dev` builds are Soufflé Nightly (com.souffle.desktop.nightly)
# so they coexist with the installed app: separate TCC rows, Dock name, and
# Application Support folder. Release builds keep com.souffle.desktop.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cfg="$root/src-tauri/tauri.nightly.conf.json"

nightly=0
for arg in "$@"; do
  if [[ "$arg" == "dev" || "$arg" == "--debug" ]]; then
    nightly=1
    break
  fi
done

if [[ "$nightly" -eq 1 ]]; then
  exec npx tauri "$@" --config "$cfg"
fi
exec npx tauri "$@"
