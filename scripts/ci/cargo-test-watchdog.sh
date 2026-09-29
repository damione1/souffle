#!/usr/bin/env bash
set -euo pipefail

# Run the already-compiled Rust test suite under a deadline. If it is still
# running at the deadline, capture `sample` stacks of every test binary into
# $SAMPLE_DIR before killing it, so a CI hang leaves evidence (SOU-132)
# instead of an empty "action has timed out".
#
# Usage: scripts/ci/cargo-test-watchdog.sh <deadline-seconds> <sample-dir> -- <cargo test args...>
# Build first with `cargo test --no-run` so the deadline covers the tests only.

if [[ $# -lt 3 || "$3" != "--" ]]; then
  echo "usage: $0 <deadline-seconds> <sample-dir> -- <cargo test args...>" >&2
  exit 2
fi

deadline="$1"
sample_dir="$2"
shift 3

cargo test "$@" &
cargo_pid=$!

elapsed=0
while kill -0 "${cargo_pid}" 2>/dev/null; do
  if (( elapsed >= deadline )); then
    echo "::error::cargo test still running after ${deadline}s; sampling test processes into ${sample_dir}" >&2
    mkdir -p "${sample_dir}"
    # Test binaries live under <target>/debug/deps/ and are children of cargo.
    for pid in $(pgrep -f '/debug/deps/' || true); do
      name="$(ps -o comm= -p "${pid}" 2>/dev/null | xargs basename 2>/dev/null || echo unknown)"
      sample "${pid}" 10 -file "${sample_dir}/sample-${name}-${pid}.txt" || true
    done
    pkill -TERM -P "${cargo_pid}" || true
    kill -TERM "${cargo_pid}" 2>/dev/null || true
    wait "${cargo_pid}" 2>/dev/null || true
    exit 124
  fi
  sleep 5
  elapsed=$((elapsed + 5))
done

wait "${cargo_pid}"
