# SOU-276 visual and validation evidence

The latest code commit is `b13d259043bb46aeb1e1e91761b17971790bc7e1`, source tree `e3fe298e85c7a0c588bf0c771994e3246c994bd1`; its review corrections and targeted evidence are recorded in the final section. The eight representatives below remain anchored to `0fd94bb3e58767dee2ca238b22b954c5cc5502a2`, validated source tree `52902ed6f604933d333bc8cd35df60674a9994a0`. Earlier commits are the original implementation `d0cb110b050fb8376f4c55b4eebd91b98d2bec36` and evidence `c86ebd649648fad84ded88ddabba7f5f6c180895`, based on `0004eced9db593aab9742c59b1e4ba2f9abc7ca9` (SOU-166). The [architecture inventory](../../typography.md) accounts for all 229 text items in 50 active Slint files, 36 roles, the native controls, and all 18 pinned Inter 4.1 faces.

The representative images below were captured on 2026-10-06 from the actually compiled, Developer ID signed diagnostic bundle at the implementation SHA. It uses the shipped MainWindow/components, Skia/Metal, process appearance and native HUD bridge with isolated synthetic data. It does not open a user database, request permissions or start recording. The actual signed product was also built and launched with the separate `com.souffle.typography.nightly.qa` identity; its onboarding image is identified separately.

Fixture executable SHA-256: `07e1164b83a90143fa2aafc009b19bd68c3eeb0551a6d3b32c8db6f2821e963f`. Product executable SHA-256: `fc6811afcf653b4bf6e6086f435d9699dc7d9022dc759d6ce606773ed5360a0b`. Signature verification: `codesign --verify --deep --strict --verbose=2` succeeded for both bundles; the fixture's signing identity is `Developer ID Application (team X6H966RSDB)`. These fixture logs and product metadata identify `0fd94bb3`, not the later review correction.

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

The earlier matrix remains separate: **184 shipped-component surface captures + 8 diagnostic specimens = 192 Slint captures**, and **32 actual native HUD captures**, from pre-review tree `bd46e20750b15ccc66905701e7f57de0a8b34a6e`. Another **56 targeted captures** cover the paragraph editor and suggestion queue after rebasing on SOU-166. The original representatives and offline runs belong to `d0cb110b`; the seven fixture representatives and actual product image above were refreshed after `0fd94bb3`, without repeating the exhaustive matrices. That latest correction changes build dependency tracking only, preserving all fonts, roles and runtime behavior. Normal screens were exercised at 680 and 1040, FR/EN, light/dark, with a client height of 820 and outer height of 852. The specimen uses a fixed 1040×780 viewport and 1040×812 outer window; its QA log reports the request, not the effective viewport. Native HUD modes keep their fixed geometries. The product screenshot uses its actual 1040×1084 outer window. Representative views of every surface category were inspected; this does not claim that every matrix image received a separate manual review.

| AC | Evidence |
| --- | --- |
| AC1 | Exhaustive architecture inventory; all text controls consume generated roles; old font assets removed |
| AC2 | Real Fontique selections and CoreText glyph runs identify all 18 embedded static faces, PostScript names and source SHA-256; specimen above |
| AC3 | Build/Contracts validator and nine typography tests; negative fixtures include underscores, imperative/compound assignments, both sides of two-way font/role aliases, local structs, missing native bindings, multiline transformations and attributed `.font: font.withSize(42)` |
| AC4 | Missing/corrupt/misnamed assets and failed registration/resolution reject explicitly; the original `d0cb110b` signed fixture and actual product rendered under deny `network*`, with no globally available Inter family; runtime is unchanged by the build correction |
| AC5 | FR/EN light/dark matrices, real dropdown/popover/input actions, keyboard and AX checks, existing live-view scroll/hitbox tests, rendered digit-cell geometry and whole-counter AX label, native last-five-line metrics |
| AC6 | Before/after inventory, central role/variant table, this signed-build evidence and complete local gate below |

All required local checks passed on the frozen `0fd94bb3` implementation source; the gate after review corrections is recorded separately below:

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

CI initially exposed a self-invalidating build dependency: Slint automatically watched the generated `OUT_DIR/typography.slint`, whose rewrite was newer than Cargo's build-start timestamp. The correction uses `compile_with_output_path` and excludes only that generated projection from the returned dependencies. All authored UI, fonts, icon, PO files, translation directory and existing Slint environment settings remain tracked. To test the first generation, the local gate removed only task-generated projections and invalidated the build script's mtime before `cargo test --workspace --no-run`. It passed at 07:54:23–07:55:18 UTC, followed immediately by the existing 300-second watchdog: `Fresh souffle-slint`, Cargo preparation 0.76s, all tests passed, complete step 15s. There was no third warm-up invocation.

Compiles used `CARGO_INCREMENTAL=0`. The final CodeQL run (`2026-10-06T07:58:58Z`–`2026-10-06T08:57:02Z`) exited 0 using an external PATH wrapper adding `--ram=8192 --max-disk-cache=4096` and redirecting only database/SARIF paths to a dedicated 32 GiB RAM disk, created without `-kernel`. The source root, script and all three suites were unchanged; the task target was parked outside the source root and restored afterward. All SARIF and logs were copied to APFS before detaching the volume. The latest gate, freshness proof and final-build logs remain in `/tmp/sou276-ci-fix-evidence`; the earlier matrices, interactions and unsuccessful historical attempts remain separately in `/tmp/sou276-evidence`, without committing caches.

The complete `make nightly` used external wrappers restricting only its exact Nightly `pkill`/`pgrep` patterns to the task worktree, and `open -n` with the separate QA identity. Repository scripts and build/signature checks were unchanged. The original main-worktree Nightly PID 18509 remained active before and after, without activation or termination. The task product was captured by its own window ID and quit through its verified process ID. No `--fresh` or user data cleanup was used.

CodeQL maintainers confirm that required intermediate results can exceed the cache setting. [Primary explanation](https://github.com/github/codeql/issues/18045#issuecomment-2490433899).

CodeQL scanned 181/181 Rust files, with 180 extracted without errors and one extraction diagnostic in unchanged `app/src/frontmost.rs:48` (`panic_2021` macro expansion). That file is byte-identical to develop. The three SARIF report successful execution and zero alerts; this does not claim extraction without diagnostics.

Interaction limits are explicit: dropdown selection, word popover typing/Escape, search typing/caret/Tab, suggestion Automatic/Accept/Dismiss, editor typing/caret, Save with the original string and Cancel were observed. Editor Save was not demonstrated transmitting the earlier modified typing; reopening reset it. Escape while the textarea held focus did not emit Cancel in that automation attempt. Existing SOU-166 bindings were preserved, with its tests included in the gate. Numeric tests check positions/sizes of text items/cells, not origins of glyphs inside each item. Arbitrary user Unicode outside Inter's repertoire retains renderer fallback. The earlier isolated CoreText coexistence probe covered pre-existing same-process URL registration only, not a globally installed Inter case. No fonts were installed globally and no user DB, preferences or TCC data were purged.

## Review corrections at b13d2590

All four CodeRabbit findings were checked and corrected: the native registration path accepts CoreText-domain `alreadyRegistered`/`duplicatedName` while retaining the original embedded CGFont; the meeting fixture has a separate localized AX label; the Swift projection is generated once directly into the merged source; and PostScript lookup uses ttf-parser's official constant re-exported by the contract.

The [CoreText proof](coretext-registration.txt) distinguishes real registration from controlled failures. On macOS 27.0 (26A428), a process-registered Inter-Regular copy with a modified ampersand advance and matching PostScript name coexisted with the embedded original; graphics registration also accepted repetitions. A graphics-registration rejection was not reproduced on this OS. Repeating URL registration returned a real CoreText 105. The original direct CGFont and CTLine glyph run retained exactly its original hmtx table despite coexistence. No fonts were installed globally. The installed SDK documents that graphics registration can reject an existing PostScript name. [Apple API reference](https://developer.apple.com/documentation/coretext/ctfontmanagerregistergraphicsfont(_:_:)).

An external Swift harness compiled the production-generated source unchanged and injected only registration outcomes: CoreText 105/305 accepted, 104/102/999 rejected, foreign-domain 105 rejected, failure without CFError rejected. Malformed bytes were rejected by the real decoding path. After injected 305, rendered runs for all eighteen faces retained their original embedded hmtx tables; glyph verification and five-line HUD metrics passed (80px measured, 82px allocated). These injected branches are not presented as actual graphics-registration failures.

The rebuilt, signed fixture at b13d2590 exposes nonempty [French](hud-meeting-fr.ax.txt) and [English](hud-meeting-en.ax.txt) HUD window/group labels and localized Stop controls, while preserving the icon-only visual meeting title. Published AX extracts contain only the application/HUD subtree. Fixture SHA-256: `1f98189fa10a60663d61b0baad7d08a4db90d12f7a7fae3d14ee69b360e8af58`.

[Actual Nightly after review](nightly-review-onboarding.png) was rebuilt and Developer ID signed at b13d2590, with strict signature verification and product SHA-256 `40f95384f7ebe91e64ffd40be06aaea0175df93faa725e8c017474cb37f0637a`. Capture uses its own CGWindow 7097, 1040×1084 exterior. Its isolated launch used `com.souffle.typography.review.qa` and its task-worktree process was quit orderly. User Nightly PID82086, from `/Users/damien/Applications/`, remained active and its bundle was not modified. The earlier eight PNGs and exhaustive matrices were preserved, not recaptured or attributed to b13d2590.

All seven build/test/UI checks after these corrections passed: sidecar tests/build, fmt, all-target clippy, 1479 workspace tests (0 failed, 17 existing ignored), translations, and complete `make nightly` at 09:43:17–09:45:46 UTC. This source ran `cargo test --workspace` directly; its gate does not claim another no-run/watchdog exercise. The previous build fix separately passed real CI Contracts 37440566848: no-run 25m41s, immediate watchdog 41s without recompilation.

CodeQL for b13d2590 is **SKIPPED / user-owned**, at the user's explicit instruction that they will run it themselves. The in-progress task analysis and its monitor were stopped orderly (exit 143); no completed SARIF or CodeQL pass is claimed for this source. Partial logs were archived before restoring the task target and ejecting only the dedicated RAM disk. The earlier successful CodeQL runs remain historical evidence for their respective source trees, not validation of b13d2590. No analysis was restarted after the user's instruction.

All targeted logs, hashes, signatures, commands and timestamped exits are in `/tmp/sou276-coderabbit-evidence`. The dedicated RAM volume was detached after archiving; the task target was restored. No source changes were made during analysis.
