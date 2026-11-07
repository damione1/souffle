#!/bin/sh
# Isolated development bundle; never opens a microphone or launches the probe.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
identity=${APPLE_SIGNING_IDENTITY:-Developer ID Application: Damien Goehrig (X6H966RSDB)}
security find-identity -v -p codesigning | rg --fixed-strings "$identity" >/dev/null || {
    echo "Developer ID signing identity missing; set APPLE_SIGNING_IDENTITY." >&2
    exit 1
}
export MACOSX_DEPLOYMENT_TARGET=13.0
export SOU277_COMMIT=$(git rev-parse HEAD)
cargo build --manifest-path app/Cargo.toml -p souffle-aec-probe
# Resolve Cargo's configured target directory, including caller's shared cache.
target=$(cargo metadata --manifest-path app/Cargo.toml --no-deps --format-version 1 | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
[ -n "$target" ] || exit 1
bundle="$root/app/target/aec-probe/Souffle AEC Probe.app"
mkdir -p "$bundle/Contents/MacOS"
cp "$target/debug/souffle-aec-probe" "$bundle/Contents/MacOS/souffle-aec-probe"
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.souffle.dev.aec-probe</string>
<key>CFBundleName</key><string>Souffle AEC Probe</string>
<key>CFBundleExecutable</key><string>souffle-aec-probe</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>NSMicrophoneUsageDescription</key><string>Record explicit SOU-277 development trials to compare microphone echo cancellation.</string>
<key>NSAudioCaptureUsageDescription</key><string>Record reference system audio for explicit SOU-277 development trials.</string>
</dict></plist>
PLIST
codesign --force --options runtime --timestamp --sign "$identity" \
    --entitlements "$root/app/aec-probe/entitlements.plist" "$bundle"
codesign --verify --strict --verbose=2 "$bundle"
"$bundle/Contents/MacOS/souffle-aec-probe" --help
printf '\nProbe bundle: %s\n' "$bundle"
