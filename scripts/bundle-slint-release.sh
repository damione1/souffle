#!/bin/bash
set -e

# Build release
cargo build --manifest-path src-tauri/Cargo.toml --release --bin souffle-slint

# Find target dir
BIN_PATH="src-tauri/target/release/souffle-slint"
if [ ! -f "$BIN_PATH" ]; then
    BIN_PATH="target/release/souffle-slint"
fi

# Create App bundle structure
APP_NAME="Souffle"
APP_DIR="src-tauri/target/release/bundle/macos/${APP_NAME}.app"
CONTENTS_DIR="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"

rm -rf "${APP_DIR}"
mkdir -p "${MACOS_DIR}"
mkdir -p "${RESOURCES_DIR}"

# Copy binary
cp "${BIN_PATH}" "${MACOS_DIR}/souffle-slint"

# Extract version from Cargo.toml
APP_VERSION=$(grep -m 1 '^version = ' src-tauri/Cargo.toml | cut -d '"' -f 2)
if [ -z "$APP_VERSION" ]; then
    APP_VERSION="0.0.0"
fi

# Create Info.plist
cat > "${CONTENTS_DIR}/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>English</string>
	<key>CFBundleDisplayName</key>
	<string>${APP_NAME}</string>
	<key>CFBundleExecutable</key>
	<string>souffle-slint</string>
	<key>CFBundleIconFile</key>
	<string>icon.icns</string>
	<key>CFBundleIdentifier</key>
	<string>com.souffle.desktop</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>${APP_NAME}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>${APP_VERSION}</string>
	<key>CFBundleVersion</key>
	<string>${APP_VERSION}</string>
	<key>NSMicrophoneUsageDescription</key>
	<string>Soufflé uses the microphone to transcribe your voice for dictation and meeting notes.</string>
	<key>NSAudioCaptureUsageDescription</key>
	<string>Soufflé captures system audio during meeting recordings to transcribe the other participants.</string>
	<key>NSCalendarsFullAccessUsageDescription</key>
	<string>Soufflé reads your calendar to show today's meetings and start transcriptions from them.</string>
	<key>NSCalendarsUsageDescription</key>
	<string>Soufflé reads your calendar to show today's meetings and start transcriptions from them.</string>
	<key>NSInputMonitoringUsageDescription</key>
	<string>Soufflé watches keyboard and mouse events to catch the single key you set as a dictation shortcut.</string>
</dict>
</plist>
EOF

# Copy Icon
if [ -f "src-tauri/icons/icon.icns" ]; then
    cp "src-tauri/icons/icon.icns" "${RESOURCES_DIR}/icon.icns"
fi

# Codesign
codesign --force --options runtime --sign "-" --entitlements src-tauri/entitlements.plist "${APP_DIR}"

# Create DMG
DMG_NAME="src-tauri/target/release/bundle/macos/${APP_NAME}.dmg"
rm -f "${DMG_NAME}"
hdiutil create -volname "${APP_NAME}" -srcfolder "${APP_DIR}" -ov -format UDZO "${DMG_NAME}"

echo "Bundled ${DMG_NAME} successfully."
