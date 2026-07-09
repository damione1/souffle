#!/usr/bin/env bash
set -euo pipefail

# Builds the souffle-mcp sidecar in release mode and copies it into
# src-tauri/binaries/ with the target-triple suffix Tauri's `externalBin`
# bundling expects (it strips the suffix again when copying into the app
# bundle). Not needed for `npm run dev` — the Settings UI handles a missing
# sidecar gracefully — but must run before `tauri build` so release bundles
# include it (wired into `beforeBuildCommand` in src-tauri/tauri.conf.json).

cd "$(dirname "${BASH_SOURCE[0]}")/.."

target_triple="$(rustc -vV | sed -n 's/^host: //p')"
if [ -z "${target_triple}" ]; then
  echo "error: could not determine host target triple from 'rustc -vV'" >&2
  exit 1
fi

bin_dir="src-tauri/binaries"
dest="${bin_dir}/souffle-mcp-${target_triple}"

echo "Building souffle-mcp sidecar for ${target_triple}..."
cargo build --manifest-path src-tauri/Cargo.toml -p souffle-mcp --release

mkdir -p "${bin_dir}"
cp "src-tauri/target/release/souffle-mcp" "${dest}"

echo "souffle-mcp sidecar ready at ${dest}"
