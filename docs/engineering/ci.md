# Continuous integration

Every PR runs **Quick checks** on Linux. The target is about five minutes,
with seven minutes as the maximum job budget, including a cold dependency
cache. A timeout fails the job; it does not count as validation.

| Event | Checks |
|---|---|
| PR or merge queue into `develop` | Quick checks |
| Push to `develop` | Quick checks |
| PR or merge queue into `master` | Quick checks and Full native checks |
| Push to `master` | Quick checks and Full native checks |
| Manual **Contracts → Run workflow** on a selected branch | Quick checks and Full native checks |

Quick checks run workspace-wide formatting, the sidecar target-directory
regression check, and Clippy plus all tests for `souffle-schema`,
`souffle-mcp` and `souffle-typography`. These cover shared SQL identifiers,
paragraph grouping, MCP database queries, bundled fonts and the UI typography
contract. They do **not** compile the app backend or the Slint frontend, run
their tests, or validate translation catalogues.

Full native checks run Clippy for every workspace target, compile and run
the entire workspace test suite with the existing hang watchdog, and validate
the extracted Slint translations. They use macOS and have a separate
75-minute timeout for cold builds. The sidecar is covered by the workspace;
prebuilding a second copy for the removed Tauri build script is unnecessary.
The translation extractor version comes from `Cargo.lock`.

Native dev and test profiles keep line tables for backtraces, rather than full
variable and type debug metadata. Assertions and overflow checks keep their
defaults. This setting is independent of Slint's generated debug information:
debug builds must still retain element ids for the headless geometry tests.

The Slint package compiles its generated UI through one shared library used
by the app and all five examples. Their existing tests stay enabled; the
library has no separate test harness because its conversion checks remain in
the app tests. The backend produces an `rlib` for its Rust consumers; the
bundle does not consume a `souffle_lib` static or dynamic library. The private
summary benchmark integration tests execute a Cargo-built utility binary
directly, so running tests cannot start a second Cargo build with a different
feature graph. `cargo run --example summary_bench` remains available.

Before promoting to `master`, wait for both checks on that PR. Branch
protection is managed separately; adding this workflow does not prohibit
merging a failing PR. For native validation earlier in development, run the
relevant local tests or launch **Contracts → Run workflow**. There is no
scheduled native build and no native rebuild on each push to `develop`.

Caches are an optimization, not a prerequisite for the quick budget. Pushes
to `develop` save the small Linux cache. Native runs on `master`, or a manual
run on `develop`, save the macOS cache for subsequent runs and promotion PRs.
PRs restore caches but do not create large per-PR entries. CodeQL runs only
when explicitly invoked locally; CI does not schedule or run CodeQL scans.

## Local validation

For Rust or Slint changes, run the full local gate before opening or updating
the PR. A green Quick checks job does not replace native compilation and
tests. Run these on an Apple Silicon Mac with the translation extractor
matching `Cargo.lock` and GNU gettext installed:

```bash
./scripts/build-mcp-sidecar.test.sh
cargo fmt --manifest-path app/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path app/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --locked --manifest-path app/Cargo.toml --workspace
./scripts/check_slint_translations.sh
```

Local CodeQL analysis is manual and is outside the pre-PR gate. Agents run
`./scripts/codeql-local.sh` only when explicitly requested. There is no
automatic CodeQL scan on pushes, pull requests, merges or a schedule.

For changes confined to CI or documentation, validate the affected workflows
with `actionlint` and run the commands changed by the diff. Rebuilding the
native workspace is unnecessary when its source and build inputs are unchanged.

## Baseline

The [2026-10-12 develop run](https://github.com/damione1/souffle/actions/runs/38200872903)
took 34m51s in its native job: Clippy 7m06s, test compilation 20m55s, and tests
4m16s. Another PR attempt hit the previous 50-minute timeout during test
compilation. The quick path reduces the work required on daily PRs; it does
not claim a complete native build can finish in five minutes.
The native performance target is 8–10 minutes with a restored dependency cache;
it must be measured with all native checks enabled before being claimed.
