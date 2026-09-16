#!/usr/bin/env bash
# Minimal .app bundling for the standalone souffle-slint binary (SOU-186).
# No tauri-bundler, no cargo-bundle - a bundle is just a directory structure
# plus an Info.plist, and this crate has no assets beyond the binary + icon.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

cargo build --manifest-path src-tauri/Cargo.toml --release -p souffle-slint

app_name="Soufflé Slint"
app_dir="src-tauri/target/release/bundle/macos/${app_name}.app"

rm -rf "$app_dir"
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources"

cp src-tauri/target/release/souffle-slint "$app_dir/Contents/MacOS/souffle-slint"

cat > "$app_dir/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>souffle-slint</string>
    <key>CFBundleIdentifier</key>
    <string>com.souffle.desktop.slint-spike</string>
    <key>CFBundleName</key>
    <string>${app_name}</string>
    <key>CFBundleIconFile</key>
    <string>icon.icns</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.0.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

if [[ -f "src-tauri/icons/icon.icns" ]]; then
  cp src-tauri/icons/icon.icns "$app_dir/Contents/Resources/icon.icns"
fi

echo "Bundle created at $app_dir"
du -sh "$app_dir"
