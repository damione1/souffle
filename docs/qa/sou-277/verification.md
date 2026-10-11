# Automatic verification — SOU-277

Build host: `Mac16,7`, macOS `27.0` (`26A428`), arm64. Deployment target:
`MACOSX_DEPLOYMENT_TARGET=13.0`. This is not physical testing on macOS 13.
No microphone/speaker capture, ignored hardware test, meeting-app trial, ASR
inference or TCC reset was run. The main checkout, release PR #379 and installed
Nightly bundle were not changed.

Commands ran from the dedicated `sou-277` worktree. Cargo reused
`CARGO_TARGET_DIR=/Users/damien/Projects/souffle/app/target` as a build cache;
CodeQL uses `CODEQL_THREADS=4`, `--ram=24576` and `--max-disk-cache=8192`
on this 48-GiB host;
the probe bundle itself is under this worktree's `app/target/aec-probe/`.

| Command | Result |
|---|---|
| `cargo test --manifest-path app/Cargo.toml -p souffle audio:: --lib` (before edits) | Passed: 223 tests, 0 failed, 3 hardware tests ignored |
| `./scripts/build-mcp-sidecar.test.sh` | Passed: target-dir resolution test |
| `./scripts/build-mcp-sidecar.sh --debug` | Passed: debug sidecar built/copied in this worktree |
| `cargo fmt --manifest-path app/Cargo.toml --all -- --check` | Passed |
| `cargo clippy --manifest-path app/Cargo.toml --workspace --all-targets -- -D warnings` | Passed; Cargo reports existing dependency `block 0.1.6` future-incompatibility notice |
| `cargo test --manifest-path app/Cargo.toml --workspace --no-run` | Passed |
| `./scripts/ci/cargo-test-watchdog.sh 300 /tmp/sou277-gate-samples-final -- --manifest-path app/Cargo.toml --workspace` | Passed: 1609 tests, 0 failed, 21 ignored across workspace test-result lines |
| `./scripts/check_slint_translations.sh` | Passed: EN and FR extracted messages translated |
| `./scripts/codeql-local.sh` | Passed: Rust, JavaScript and Actions suites completed; 0 results in each SARIF report |
| `cargo test --manifest-path app/Cargo.toml -p souffle-aec-probe` | Passed: 12 tests, 0 failed, 0 ignored |
| `sh -n scripts/build-aec-probe.sh` | Passed |
| `./scripts/build-aec-probe.sh` | Passed: built, Developer-ID-signed, strict codesign verification, help launch |
| Signed executable `capture --help` | Passed without capture |
| Signed executable `init` followed by capture with incomplete protocol | Refused before hardware startup, as required |
| Signed executable `report` on empty QA directory | `complete:false`, explicit missing trials |
| `git diff --check` | Passed |

Probe tests cover partial-frame baseline replay, silence/empty ASR semantics,
normalization (including decomposed Unicode accents), ambiguous shared words,
separate edit counts, median/range, missing trials/ASR, nonfinite/empty WAVs,
native-rate WAV roundtrip, and incomplete protocol / overwrite refusal.
These are deterministic software tests, not substitute acoustic measurements.

Local logs are `/tmp/sou277-baseline.log`, `/tmp/sou277-gate-sidecar-*.log`,
`/tmp/sou277-gate-{fmt,clippy,test-build,tests,translations}-final.log`,
`/tmp/sou277-gate-codeql-final.log` and `/tmp/sou277-probe-bundle-final.log`.
Private measurement files remain outside the repository; there are no captured
WAVs or transcripts in this change.

CodeQL resource options were applied without modifying the repository script or
skipping any database/suite. The exact Bash forwarding function used was:

```bash
codeql() {
    if [ "$1" = database ] && [ "$2" = analyze ]; then
        /opt/homebrew/bin/codeql "$@" --ram=24576 --max-disk-cache=8192
    else
        /opt/homebrew/bin/codeql "$@"
    fi
}
export -f codeql
CODEQL_THREADS=4 ./scripts/codeql-local.sh
```

Intermediate scans were stopped while final review fixed the normalization,
teardown diagnostics and typed artifact contract; another scan using the default
RAM budget was superseded after observing sustained cache eviction. Only the
completed final scan is counted as validation.

The final Rust extraction archive was compared byte-for-byte with all five probe
Rust files and the modified `system_tap.rs`; all matched. CodeQL reported an
existing macro-expansion warning at `app/src/frontmost.rs:48:9` (`panic_2021`),
which is outside this change. The three completed SARIF reports contain no
security findings.
