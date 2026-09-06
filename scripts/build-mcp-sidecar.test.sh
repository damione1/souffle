#!/usr/bin/env bash
set -euo pipefail

# Cheap assertions for scripts/build-mcp-sidecar.sh: cargo metadata honors
# CARGO_TARGET_DIR, and the script asks cargo rather than hardcoding
# src-tauri/target. Does not build the sidecar.

cd "$(dirname "${BASH_SOURCE[0]}")/.."

script="./scripts/build-mcp-sidecar.sh"

if grep -q 'src-tauri/target/release/souffle-mcp' "${script}"; then
  echo "fail: ${script} still hardcodes src-tauri/target/release/souffle-mcp" >&2
  exit 1
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

abs="${tmpdir}/abs-target"
got="$(CARGO_TARGET_DIR="${abs}" "${script}" --print-target-dir)"
if [ "${got}" != "${abs}" ]; then
  echo "fail: expected CARGO_TARGET_DIR ${abs}, got: ${got}" >&2
  exit 1
fi

rel_dir="mcp-sidecar-rel-target-$$"
cleanup_rel() {
  rm -rf "${tmpdir}" "${rel_dir}"
}
trap cleanup_rel EXIT

got_rel="$(CARGO_TARGET_DIR="${rel_dir}" "${script}" --print-target-dir)"
expected_rel="$(pwd)/${rel_dir}"
if [ "${got_rel}" != "${expected_rel}" ]; then
  echo "fail: expected relative CARGO_TARGET_DIR to resolve to ${expected_rel}, got: ${got_rel}" >&2
  exit 1
fi

echo "ok: MCP sidecar target dir resolves via cargo metadata"
