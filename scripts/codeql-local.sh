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

# Suites are named by pack spec, not by bare name. The bare name only
# resolves inside the CodeQL Action's bundle, which ships the query packs; the
# Homebrew CLI carries none, so it downloads them into ~/.codeql/packages and
# resolves them by spec.
codeql pack download codeql/rust-queries codeql/javascript-queries codeql/actions-queries

codeql database analyze "$out/rust" \
  'codeql/rust-queries:codeql-suites/rust-code-scanning.qls' \
  --format=sarif-latest --output="$out/rust.sarif"
codeql database analyze "$out/js" \
  'codeql/javascript-queries:codeql-suites/javascript-code-scanning.qls' \
  --format=sarif-latest --output="$out/js.sarif"
codeql database analyze "$out/actions" \
  'codeql/actions-queries:codeql-suites/actions-code-scanning.qls' \
  --format=sarif-latest --output="$out/actions.sarif"

echo "CodeQL local gate green. SARIF under $out/*.sarif"
