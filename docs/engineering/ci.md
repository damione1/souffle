# Continuous integration

Every PR runs **Quick checks** on Linux. The target is about five minutes,
with seven minutes as the maximum job budget, including a cold dependency
cache. A timeout fails the job; it does not count as validation.

| Event | Checks |
|---|---|
| PR or merge queue into `develop` | Quick checks |
| Push to `develop` | Quick checks, plus the existing CodeQL workflow |
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

Before promoting to `master`, wait for both checks on that PR. Branch
protection is managed separately; adding this workflow does not prohibit
merging a failing PR. For native validation earlier in development, run the
relevant local tests or launch **Contracts → Run workflow**. There is no
scheduled native build and no native rebuild on each push to `develop`.

Caches are an optimization, not a prerequisite for the quick budget. Pushes
to `develop` save the small Linux cache. Native runs on `master`, or a manual
run on `develop`, save the macOS cache for subsequent runs and promotion PRs.
PRs restore caches but do not create large per-PR entries. CodeQL remains
on pushes to `develop`, outside the PR workflow.

## Baseline

The [2026-10-12 develop run](https://github.com/damione1/souffle/actions/runs/38200872903)
took 34m51s in its native job: Clippy 7m06s, test compilation 20m55s, and tests
4m16s. Another PR attempt hit the previous 50-minute timeout during test
compilation. The quick path reduces the work required on daily PRs; it does
not claim a complete native build can finish in five minutes.
