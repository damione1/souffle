#!/usr/bin/env bash
set -euo pipefail

APP="$(ls -d /Users/damien/Projects/souffle-SOU-218-repair/src-tauri/target/debug/bundle/macos/Souff*Nightly.app | head -n 1)"
OUT_DIR="/tmp/souffle_screenshots"
DB="$HOME/Library/Application Support/com.souffle.desktop/souffle.db"
DATA_DIR="$HOME/Library/Application Support/com.souffle.desktop"

# We already have settings_en.png because it's the default!
# Now let's hack main.rs to force French locale so we can capture settings_fr.png!

# Make settings open to interface by default
sed -i '' 's/in-out property <SettingsTab> settings-tab: SettingsTab.transcription;/in-out property <SettingsTab> settings-tab: SettingsTab.interface;/g' src-tauri/souffle-slint/ui/main_window.slint
sed -i '' 's/in-out property <bool> settings-open: false;/in-out property <bool> settings-open: true;/g' src-tauri/souffle-slint/ui/main_window.slint

# Force AppLocale::Fr
sed -i '' 's/select_app_locale(locale);/select_app_locale(crate::AppLocale::Fr);/g' src-tauri/souffle-slint/src/main.rs

# Rebuild the bundle
./scripts/bundle-macos.sh --debug --nightly

rm -rf "$DATA_DIR" || true
mkdir -p "$DATA_DIR"
touch "$DATA_DIR/.permissions_onboarded"
touch "$DATA_DIR/.setup_onboarded"

pkill -9 -f "Contents/MacOS/souffle" || true
open "$APP"
sleep 5

./scripts/ui-shot.sh "$APP" "$OUT_DIR/settings_fr.png" 2
pkill -9 -f "Contents/MacOS/souffle" || true

# Revert hacks
git checkout src-tauri/souffle-slint/ui/main_window.slint
git checkout src-tauri/souffle-slint/src/main.rs
