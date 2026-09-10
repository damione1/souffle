#!/usr/bin/env bash
# Local CodeQL gate (replaces PR CI). Same languages / build-mode as
# .github/workflows/codeql.yml. Requires the CodeQL CLI on PATH:
#   brew install --cask codeql
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if ! command -v codeql >/dev/null 2>&1; then
  echo "codeql not on PATH. Install with: brew install --cask codeql" >&2
  exit 1
fi

out=".codeql"
rm -rf "$out"
mkdir -p "$out"

# Match CI: build-mode none (no cargo build) for rust; extractors for js/actions.
codeql database create "$out/rust" \
  --language=rust --build-mode=none --source-root=.
codeql database create "$out/js" \
  --language=javascript-typescript --source-root=.
codeql database create "$out/actions" \
  --language=actions --source-root=.

codeql database analyze "$out/rust" rust-code-scanning.qls \
  --format=sarif-latest --output="$out/rust.sarif"
codeql database analyze "$out/js" javascript-code-scanning.qls \
  --format=sarif-latest --output="$out/js.sarif"
codeql database analyze "$out/actions" actions-code-scanning.qls \
  --format=sarif-latest --output="$out/actions.sarif"

echo "CodeQL local gate green. SARIF under $out/*.sarif"
