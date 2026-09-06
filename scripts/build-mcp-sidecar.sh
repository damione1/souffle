#!/usr/bin/env bash
set -euo pipefail

# Builds the souffle-mcp sidecar in release mode and copies it into
# src-tauri/binaries/ with the target-triple suffix Tauri's `externalBin`
# bundling expects (it strips the suffix again when copying into the app
# bundle). Not needed for `npm run dev` — the Settings UI handles a missing
# sidecar gracefully — but must run before `tauri build` so release bundles
# include it (wired into `beforeBuildCommand` in src-tauri/tauri.conf.json).
#
# Cargo artifacts are not always at src-tauri/target. CARGO_TARGET_DIR and
# .cargo/config.toml `build.target-dir` relocate them. Ask cargo metadata.

cd "$(dirname "${BASH_SOURCE[0]}")/.."

cargo_target_dir() {
  cargo metadata --format-version 1 --manifest-path src-tauri/Cargo.toml --no-deps \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])'
}

if [ "${1:-}" = "--print-target-dir" ]; then
  cargo_target_dir
  exit 0
fi

target_triple="$(rustc -vV | sed -n 's/^host: //p')"
if [ -z "${target_triple}" ]; then
  echo "error: could not determine host target triple from 'rustc -vV'" >&2
  exit 1
fi

bin_dir="src-tauri/binaries"
dest="${bin_dir}/souffle-mcp-${target_triple}"

echo "Building souffle-mcp sidecar for ${target_triple}..."
cargo build --manifest-path src-tauri/Cargo.toml -p souffle-mcp --release

target_dir="$(cargo_target_dir)"
src="${target_dir}/release/souffle-mcp"
if [ ! -f "${src}" ]; then
  echo "error: souffle-mcp not found at ${src}" >&2
  echo "error: cargo's target directory may have been relocated via CARGO_TARGET_DIR or .cargo/config.toml build.target-dir" >&2
  exit 1
fi

mkdir -p "${bin_dir}"
cp "${src}" "${dest}"

echo "souffle-mcp sidecar ready at ${dest}"
