#!/usr/bin/env bash
# Build, sign, (optionally) notarize, and package Soufflé as a macOS .app +
# .dmg, without tauri-bundler/tauri-build (SOU-192).
#
# A macOS app bundle is just a directory structure plus an Info.plist; this
# script assembles it by hand, replicating exactly what
# `tauri.conf.json`'s `bundle` section + `tauri-apps/tauri-action` used to do:
#   - binary + MCP sidecar + resources (VAD model, sounds) + the ONNX Runtime
#     dylib (as a Framework, so it gets code-signed — notarization rejects
#     any unsigned Mach-O anywhere in the bundle)
#   - Info.plist merged from the base template below + src-tauri/Info.plist's
#     usage-description keys (NSMicrophoneUsageDescription etc.)
#   - entitlements + hardened runtime, signed with a real Developer ID
#     identity (never ad hoc `--sign -`: TCC records key off the signing
#     identity's designated requirement, and an ad hoc signature's DR is the
#     binary's own cdhash — useless the moment the binary is rebuilt)
#   - optional notarization + stapling (skipped, not skipped-and-silent, when
#     credentials are absent — matches release.yml's existing "signing is
#     optional" comment)
#   - a .dmg, same layout make nightly-dmg already produced
#
# Usage:
#   scripts/bundle-macos.sh [--debug] [--nightly] [--dmg] [--notarize] [--sign-updater]
#
#   --debug          debug cargo profile (default: release)
#   --nightly        "Soufflé Nightly" / com.souffle.desktop.nightly, coexists
#                     with the installed app (mirrors tauri.nightly.conf.json)
#   --dmg            also wrap the signed .app in a .dmg
#   --notarize       submit to Apple notarization + staple (needs APPLE_ID,
#                     APPLE_PASSWORD, APPLE_TEAM_ID; release builds only —
#                     refuses on --debug, matching run-nightly.sh's own
#                     "no notarization needed for local builds" note)
#   --sign-updater   also produce the signed update artifact (.tar.gz +
#                     .tar.gz.sig) and latest.json for the GitHub release
#                     (needs TAURI_SIGNING_PRIVATE_KEY[_PASSWORD])
#
# Env:
#   APPLE_SIGNING_IDENTITY   Developer ID Application identity (falls back to
#                            the same default run-nightly.sh uses)
#   APPLE_ID, APPLE_PASSWORD, APPLE_TEAM_ID   notarytool credentials
#   TAURI_SIGNING_PRIVATE_KEY[_PASSWORD]      minisign key for --sign-updater
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

profile="release"
nightly=0
dmg_mode=0
notarize=0
sign_updater=0
for arg in "$@"; do
  case "$arg" in
    --debug) profile="debug" ;;
    --nightly) nightly=1 ;;
    --dmg) dmg_mode=1 ;;
    --notarize) notarize=1 ;;
    --sign-updater) sign_updater=1 ;;
    *)
      echo "error: unknown argument: $arg" >&2
      exit 1
      ;;
  esac
done

if [[ "$notarize" -eq 1 && "$profile" == "debug" ]]; then
  echo "error: --notarize needs --release (a debug build has no quarantine flag to clear anyway)" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Identity, naming
# ---------------------------------------------------------------------------

app_version="$(sed -n 's/^version = "\(.*\)"/\1/p' src-tauri/Cargo.toml | head -1)"

if [[ "$nightly" -eq 1 ]]; then
  app_name="Soufflé Nightly"
  bundle_id="com.souffle.desktop.nightly"
  entitlements="src-tauri/entitlements.nightly.plist"
  [[ -f "$entitlements" ]] || entitlements="src-tauri/entitlements.plist"
else
  app_name="Soufflé"
  bundle_id="com.souffle.desktop"
  entitlements="src-tauri/entitlements.plist"
fi

export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:-Developer ID Application: Damien Goehrig (X6H966RSDB)}"
if ! security find-identity -v -p codesigning 2>/dev/null | grep -qF "$APPLE_SIGNING_IDENTITY"; then
  echo "WARNING: no codesigning identity matching '$APPLE_SIGNING_IDENTITY' in the keychain." >&2
  echo "         Building unsigned: TCC will treat every rebuild as a new subject, and" >&2
  echo "         notarization (if requested) will fail outright." >&2
  unset APPLE_SIGNING_IDENTITY
fi

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

echo "==> Building souffle-slint (${profile})"
if [[ "$profile" == "release" ]]; then
  cargo build --manifest-path src-tauri/Cargo.toml --release -p souffle-slint
  ./scripts/build-mcp-sidecar.sh
else
  cargo build --manifest-path src-tauri/Cargo.toml -p souffle-slint
  ./scripts/build-mcp-sidecar.sh --debug
fi

bin_src="src-tauri/target/${profile}/souffle-slint"
target_triple="$(rustc -vV | sed -n 's/^host: //p')"
sidecar_src="src-tauri/binaries/souffle-mcp-${target_triple}"

# ---------------------------------------------------------------------------
# Assemble the bundle
# ---------------------------------------------------------------------------

bundle_root="src-tauri/target/${profile}/bundle/macos"
app_dir="${bundle_root}/${app_name}.app"
contents="${app_dir}/Contents"

echo "==> Assembling ${app_dir}"
rm -rf "$app_dir"
mkdir -p "$contents/MacOS" "$contents/Resources/resources" "$contents/Frameworks"

cp "$bin_src" "$contents/MacOS/souffle"
if [[ -f "$sidecar_src" ]]; then
  cp "$sidecar_src" "$contents/MacOS/souffle-mcp"
else
  echo "WARNING: MCP sidecar not found at $sidecar_src; Settings > MCP will report it missing" >&2
fi

cp src-tauri/resources/silero_vad_v4.onnx "$contents/Resources/resources/silero_vad_v4.onnx"
mkdir -p "$contents/Resources/resources/sounds"
cp src-tauri/resources/sounds/dictation_start.wav "$contents/Resources/resources/sounds/dictation_start.wav"
cp src-tauri/resources/sounds/dictation_stop.wav "$contents/Resources/resources/sounds/dictation_stop.wav"
# In Frameworks, not Resources: notarization rejects an unsigned Mach-O
# anywhere in the bundle, and only Frameworks/MacOS get individually signed
# below (see ort_runtime.rs's resolve_resource for the matching lookup order).
cp src-tauri/resources/libonnxruntime.dylib "$contents/Frameworks/libonnxruntime.dylib"

cp src-tauri/icons/icon.icns "$contents/Resources/icon.icns"

# Info.plist: base bundle keys, plus every usage-description key from
# src-tauri/Info.plist (the file `tauri_build::build()` used to merge in
# automatically). Merged with PlistBuddy rather than hand-formatted XML so a
# key added to src-tauri/Info.plist later is picked up without touching this
# script.
cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>souffle</string>
    <key>CFBundleIdentifier</key>
    <string>${bundle_id}</string>
    <key>CFBundleName</key>
    <string>${app_name}</string>
    <key>CFBundleDisplayName</key>
    <string>${app_name}</string>
    <key>CFBundleIconFile</key>
    <string>icon.icns</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${app_version}</string>
    <key>CFBundleVersion</key>
    <string>${app_version}</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

if [[ -f "src-tauri/Info.plist" ]]; then
  python3 - "src-tauri/Info.plist" "$contents/Info.plist" <<'PY'
import plistlib
import sys

extra_path, target_path = sys.argv[1], sys.argv[2]
with open(extra_path, "rb") as f:
    extra = plistlib.load(f)
with open(target_path, "rb") as f:
    target = plistlib.load(f)
target.update(extra)
with open(target_path, "wb") as f:
    plistlib.dump(target, f)
PY
fi

# ---------------------------------------------------------------------------
# Code signing (inside-out: nested Mach-O first, then the bundle as a whole)
# ---------------------------------------------------------------------------

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  echo "==> Signing with: $APPLE_SIGNING_IDENTITY"
  codesign --force --options runtime --timestamp \
    --sign "$APPLE_SIGNING_IDENTITY" "$contents/Frameworks/libonnxruntime.dylib"
  if [[ -f "$contents/MacOS/souffle-mcp" ]]; then
    codesign --force --options runtime --timestamp \
      --sign "$APPLE_SIGNING_IDENTITY" "$contents/MacOS/souffle-mcp"
  fi
  codesign --force --options runtime --timestamp \
    --entitlements "$entitlements" \
    --sign "$APPLE_SIGNING_IDENTITY" "$contents/MacOS/souffle"
  codesign --force --options runtime --timestamp --deep \
    --entitlements "$entitlements" \
    --sign "$APPLE_SIGNING_IDENTITY" "$app_dir"
  codesign --verify --deep --strict --verbose=2 "$app_dir"
else
  echo "==> Skipping code signing (no identity available)"
fi

echo "Bundle created at $app_dir"
du -sh "$app_dir"

# ---------------------------------------------------------------------------
# Notarization (release builds only, credentials required)
# ---------------------------------------------------------------------------

if [[ "$notarize" -eq 1 ]]; then
  if [[ -z "${APPLE_ID:-}" || -z "${APPLE_PASSWORD:-}" || -z "${APPLE_TEAM_ID:-}" ]]; then
    echo "error: --notarize needs APPLE_ID, APPLE_PASSWORD, APPLE_TEAM_ID" >&2
    exit 1
  fi
  echo "==> Notarizing (this talks to Apple's servers and can take several minutes)"
  zip_path="${bundle_root}/${app_name}.zip"
  rm -f "$zip_path"
  ditto -c -k --keepParent "$app_dir" "$zip_path"
  xcrun notarytool submit "$zip_path" \
    --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" \
    --wait
  xcrun stapler staple "$app_dir"
  rm -f "$zip_path"
fi

# ---------------------------------------------------------------------------
# DMG
# ---------------------------------------------------------------------------

if [[ "$dmg_mode" -eq 1 ]]; then
  dmg_dir="src-tauri/target/${profile}/bundle/dmg"
  dmg_path="${dmg_dir}/${app_name}.dmg"
  echo "==> Creating ${dmg_path}"
  mkdir -p "$dmg_dir"
  stage="$(mktemp -d)"
  cp -R "$app_dir" "$stage/"
  ln -s /Applications "$stage/Applications"
  rm -f "$dmg_path"
  hdiutil create -volname "$app_name" -srcfolder "$stage" -ov -format UDZO "$dmg_path"
  rm -rf "$stage"
  echo "DMG created at $dmg_path"
fi

# ---------------------------------------------------------------------------
# Update artifact (release builds only): .tar.gz + minisign signature +
# latest.json, same format native::updater.rs verifies and the same
# manifest shape tauri-plugin-updater's endpoint used to serve.
# ---------------------------------------------------------------------------

if [[ "$sign_updater" -eq 1 ]]; then
  if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
    echo "error: --sign-updater needs TAURI_SIGNING_PRIVATE_KEY" >&2
    exit 1
  fi
  if ! command -v minisign >/dev/null 2>&1; then
    echo "error: --sign-updater needs the minisign CLI (brew install minisign)" >&2
    exit 1
  fi
  update_dir="src-tauri/target/${profile}/bundle/updater"
  mkdir -p "$update_dir"
  tar_path="${update_dir}/${app_name// /-}.app.tar.gz"
  echo "==> Building update artifact ${tar_path}"
  tar -C "$bundle_root" -czf "$tar_path" "${app_name}.app"

  key_file="$(mktemp)"
  trap 'rm -f "$key_file"' EXIT
  printf '%s' "$TAURI_SIGNING_PRIVATE_KEY" > "$key_file"
  if [[ -n "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" ]]; then
    minisign -S -s "$key_file" -m "$tar_path" <<< "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
  else
    minisign -S -s "$key_file" -m "$tar_path" <<< ""
  fi

  sig_b64="$(base64 -i "${tar_path}.sig")"
  notes="$(git tag -l --format='%(contents)' "v${app_version}" 2>/dev/null || true)"
  cat > "${update_dir}/latest.json" <<JSON
{
  "version": "${app_version}",
  "notes": $(printf '%s' "$notes" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))'),
  "pub_date": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "platforms": {
    "darwin-aarch64": {
      "signature": "${sig_b64}",
      "url": "https://github.com/damione1/souffle/releases/download/v${app_version}/$(basename "$tar_path")"
    }
  }
}
JSON
  echo "Update artifact + latest.json ready in $update_dir"
fi
