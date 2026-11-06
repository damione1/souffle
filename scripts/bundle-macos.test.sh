#!/usr/bin/env bash
# Exercise QA build guards and Cargo arguments without building or signing.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
mkdir "$tmpdir/bin"
export BUNDLE_TEST_LOG="$tmpdir/cargo.log"
export APPLE_SIGNING_IDENTITY='Test Developer Identity'
unset SLINT_EMIT_DEBUG_INFO
cat > "$tmpdir/bin/security" <<'SH'
#!/bin/sh
printf '%s\n' "$APPLE_SIGNING_IDENTITY"
SH
cat > "$tmpdir/bin/cargo" <<'SH'
#!/bin/sh
printf 'debug=%s\n' "${SLINT_EMIT_DEBUG_INFO-unset}" > "$BUNDLE_TEST_LOG"
printf '%s\n' "$@" >> "$BUNDLE_TEST_LOG"
exit 83
SH
chmod +x "$tmpdir/bin/security" "$tmpdir/bin/cargo"
export PATH="$tmpdir/bin:$PATH"

expect_rejected() {
  local message="$1"
  shift
  rm -f "$BUNDLE_TEST_LOG"
  local status=0
  ./scripts/bundle-macos.sh "$@" > "$tmpdir/output" 2>&1 || status=$?
  if [[ "$status" -ne 1 ]] || ! grep -qF -- "$message" "$tmpdir/output"; then
    cat "$tmpdir/output" >&2
    echo "fail: expected preflight rejection for $*" >&2
    exit 1
  fi
  [[ ! -e "$BUNDLE_TEST_LOG" ]] || { echo 'fail: invalid flags invoked Cargo' >&2; exit 1; }
}

expect_rejected '--slint-mcp requires --debug --nightly' --slint-mcp
expect_rejected '--slint-mcp requires --debug --nightly' --nightly --slint-mcp
expect_rejected '--slint-mcp requires --debug --nightly' --debug --slint-mcp
expect_rejected '--slint-mcp cannot produce an updater artifact' --debug --nightly --slint-mcp --sign-updater
expect_rejected '--native-aec requires --nightly' --debug --native-aec

expect_build() {
  local debug="$1" features="$2"
  shift 2
  local status=0
  ./scripts/bundle-macos.sh --debug --nightly "$@" > "$tmpdir/output" 2>&1 || status=$?
  [[ "$status" -eq 83 ]] || { cat "$tmpdir/output" >&2; exit 1; }
  printf 'debug=%s\nbuild\n--manifest-path\napp/Cargo.toml\n-p\nsouffle-slint\n' "$debug" > "$tmpdir/expected"
  if [[ -n "$features" ]]; then
    printf '%s\n' "$features" >> "$tmpdir/expected"
  fi
  cmp "$tmpdir/expected" "$BUNDLE_TEST_LOG" || { cat "$BUNDLE_TEST_LOG" >&2; exit 1; }
}

expect_build unset ''
expect_build unset $'--features\nexperimental-native-aec' --native-aec
expect_build 1 $'--features\nslint/mcp' --slint-mcp
expect_build 1 $'--features\nexperimental-native-aec\n--features\nslint/mcp' --native-aec --slint-mcp
echo 'ok: QA flags are guarded and only explicit MCP builds enable Slint inspection'
