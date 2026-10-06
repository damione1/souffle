# SOU-276 visual and validation evidence

The implementation commit is `d0cb110b050fb8376f4c55b4eebd91b98d2bec36`, based on `0004eced9db593aab9742c59b1e4ba2f9abc7ca9` (SOU-166). Its validated source tree is `64eaff810d58308240760a84c58ad331a240d73b`. This separate evidence commit adds documentation and PNGs only. The [architecture inventory](../../typography.md) accounts for all 229 text items in 50 active Slint files, 36 roles, the native controls, and all 18 pinned Inter 4.1 faces.

The representative images below were captured on 2026-10-06 from the actually compiled, Developer ID signed diagnostic bundle at the implementation SHA. It uses the shipped MainWindow/components, Skia/Metal, process appearance and native HUD bridge with isolated synthetic data. It does not open a user database, request permissions or start recording. The actual signed product was also built and launched with the separate `com.souffle.typography.nightly.qa` identity; its onboarding image is identified separately.

Fixture executable SHA-256: `7ad6579b5923b8c8f94fd8d7035287107661e34dca610e534ce1f4b0bb781a3e`. Product executable SHA-256: `b04dfacd57c07d346fc5b906d0a3c08c92a8c7e87f53d9ac2ca410814233dd07`. Signature verification: `codesign --verify --deep --strict --verbose=2` succeeded for both bundles; the fixture's signing identity is `Developer ID Application (team X6H966RSDB)`.

| Image | Locale / theme / width | Proof |
| --- | --- | --- |
| [Home](home-fr-light-680.png) | FR / light / 680 | Timeline, shortcuts, ampersand, diacritics and counters |
| [Detail](detail-en-dark-1040.png) | EN / dark / 1040 | Transcript, summary and long text |
| [Specimen](specimen-fr-light-1040.png) | FR / light / 1040 | Nine static weights, upright and true italic, all reference glyphs |
| [Expanded HUD](hud-expanded-en-dark.png) | EN / dark / native 440×154 | Actual AppKit text and retained recent lines |
| [Compact HUD](hud-compact-fr-light.png) | FR / light / native 280×64 | Actual compact dictation label and controls |
| [Paragraph editor](editor-fr-light-680.png) | FR / light / 680 | SOU-166 textarea, actions and central error role |
| [Suggestion queue](suggestions-en-dark-680.png) | EN / dark / 680 | SOU-166 learning modes, rows and actions |
| [Actual Nightly](nightly-onboarding.png) | EN / light / actual product | Signed product startup with custom window chrome |

The earlier matrix remains separate: **184 shipped-component surface captures + 8 diagnostic specimens = 192 Slint captures**, and **32 actual native HUD captures**, from pre-review tree `bd46e20750b15ccc66905701e7f57de0a8b34a6e`. Another **56 targeted captures** cover the paragraph editor and suggestion queue after rebasing on SOU-166. The seven fixture representatives and actual product image above were rebuilt after the final code commit; the earlier matrices were not built from that commit. Normal screens were exercised at 680 and 1040, FR/EN, light/dark, with a client height of 820 and outer height of 852. The specimen uses a fixed 1040×780 viewport and 1040×812 outer window; its QA log reports the request, not the effective viewport. Native HUD modes keep their fixed geometries. The product screenshot uses its actual 1040×1084 outer window. Representative views of every surface category were inspected; this does not claim that every matrix image received a separate manual review.

| AC | Evidence |
| --- | --- |
| AC1 | Exhaustive architecture inventory; all text controls consume generated roles; old font assets removed |
| AC2 | Real Fontique selections and CoreText glyph runs identify all 18 embedded static faces, PostScript names and source SHA-256; specimen above |
| AC3 | Build/Contracts validator and nine typography tests; negative fixtures include underscores, imperative/compound assignments, both sides of two-way font/role aliases, local structs, missing native bindings, multiline transformations and attributed `.font: font.withSize(42)` |
| AC4 | Missing/corrupt/misnamed assets and failed registration/resolution reject explicitly; signed fixture and actual product rendered under deny `network*`, with no globally available Inter family |
| AC5 | FR/EN light/dark matrices, real dropdown/popover/input actions, keyboard and AX checks, existing live-view scroll/hitbox tests, rendered digit-cell geometry and whole-counter AX label, native last-five-line metrics |
| AC6 | Before/after inventory, central role/variant table, this signed-build evidence and complete local gate below |

All required local checks passed on the frozen implementation source:

| Command | Result |
| --- | --- |
| `./scripts/build-mcp-sidecar.test.sh` | PASS |
| `./scripts/build-mcp-sidecar.sh --debug` | PASS |
| `cargo fmt --manifest-path app/Cargo.toml --all -- --check` | PASS |
| `cargo clippy --manifest-path app/Cargo.toml --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --manifest-path app/Cargo.toml --workspace` | PASS: 1479 passed, 0 failed, 17 existing ignored |
| `./scripts/check_slint_translations.sh` | PASS |
| `CODEQL_THREADS=2 ./scripts/codeql-local.sh` | PASS: all Rust/JavaScript/Actions code-scanning suites; zero alerts in each SARIF |
| `make nightly` | PASS: built, signed and launched isolated product |

Compiles used `CARGO_INCREMENTAL=0`. The final CodeQL run (`2026-10-06T05:55:33Z`–`2026-10-06T06:50:49Z`) used an external PATH wrapper adding `--ram=8192 --max-disk-cache=4096` and redirecting only database/SARIF paths to a dedicated 32 GiB RAM disk, created without `-kernel`. The source root, script and all three suites were unchanged; the task target was parked outside the source root and restored afterward. All SARIF and logs were copied to APFS before detaching the volume. Earlier interrupted or failed attempts are not passes; their exact status and resource history, full logs, matrices and interaction records remain in `/tmp/sou276-evidence`, without committing caches.

CodeQL maintainers confirm that required intermediate results can exceed the cache setting. [Primary explanation](https://github.com/github/codeql/issues/18045#issuecomment-2490433899).

CodeQL scanned 181/181 Rust files, with 180 extracted without errors and one extraction diagnostic in unchanged `app/src/frontmost.rs:48` (`panic_2021` macro expansion). That file is byte-identical to develop. The three SARIF report successful execution and zero alerts; this does not claim extraction without diagnostics.

Interaction limits are explicit: dropdown selection, word popover typing/Escape, search typing/caret/Tab, suggestion Automatic/Accept/Dismiss, editor typing/caret, Save with the original string and Cancel were observed. Editor Save was not demonstrated transmitting the earlier modified typing; reopening reset it. Escape while the textarea held focus did not emit Cancel in that automation attempt. Existing SOU-166 bindings were preserved, with its tests included in the gate. Numeric tests check positions/sizes of text items/cells, not origins of glyphs inside each item. Arbitrary user Unicode outside Inter's repertoire retains renderer fallback. The isolated CoreText coexistence probe covers pre-existing same-process URL registration only, not a globally installed Inter case. No fonts were installed globally and no user DB, preferences or TCC data were purged.
