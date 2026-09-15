#!/bin/bash
set -e

# Build the release binary
cargo build --manifest-path src-tauri/Cargo.toml --release --bin souffle-slint

# Create the bundle structure
APP_NAME="Soufflé Slint"
APP_DIR="src-tauri/target/release/bundle/osx/${APP_NAME}.app"

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS"
mkdir -p "$APP_DIR/Contents/Resources"

# Copy binary
cp src-tauri/target/release/souffle-slint "$APP_DIR/Contents/MacOS/souffle"

# Create minimal Info.plist
cat > "$APP_DIR/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>souffle</string>
    <key>CFBundleIdentifier</key>
    <string>com.souffle.slint</string>
    <key>CFBundleName</key>
    <string>Soufflé Slint</string>
    <key>CFBundleIconFile</key>
    <string>icon.icns</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
</dict>
</plist>
EOF

# Copy icon if exists
if [ -f "src-tauri/icons/icon.icns" ]; then
    cp src-tauri/icons/icon.icns "$APP_DIR/Contents/Resources/icon.icns"
fi

echo "Bundle created at $APP_DIR"

# Print bundle size
du -sh "$APP_DIR"
