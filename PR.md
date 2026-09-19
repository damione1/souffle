chore(packaging): SOU-192 delete retired tauri frontend and residue

This is the final epic ticket for SOU-185/SOU-192. It completely deletes the retired Tauri-era frontend, matching the required ACs to safely remove npm residue from the shipped app.

## Acceptance criteria

1. **Suppression définitive de src/** — verified manually: `src/` and `public/` directories deleted.
2. **Suppression de package.json et autres configs TS/Vite/Svelte** — verified manually: `package.json`, `package-lock.json`, `tsconfig.*`, `vite.config.ts`, `vitest.config.ts`, `playwright.config.ts` deleted.
3. **Mise à jour des workflows** — verified manually: `.github/workflows/e2e.yml` deleted, `.github/workflows/contracts.yml` modified to remove frontend checks.
4. **Passage de la porte locale** — verified by absence of `npm run check`, passing `cargo fmt --check`, `cargo clippy`, `cargo test`, and `codeql`.
5. **Aucune référence résiduelle à tauri, specta, generated.ts** — verified by absence of symbol in codebase, checked via `grep -rniE "specta|tauri|npm|generated\.ts" . --exclude-dir=.git`. Cleaned `AGENTS.md`, `CLAUDE.md`, `.coderabbit.yaml`, and `src-tauri/Cargo.toml`.
6. **Suppression des dépendances orphelines** — verified by absence of unused dependencies in `cargo-machete` output (removed `candle-transformers`, `log`, `sha2`, `chrono`).

## Out of scope

- `src-tauri/` itself is retained (it contains the Slint Rust UI and backend).
- `site/` eleventy app is retained (just replaced `tauri dev` with `make nightly`).

## Risk zones

- `db::snippets::tests::fold_trigger_matches_fixtures` requires the `snippet-fold-test-cases.json` fixture, which was previously in `src/`. Restored and moved to `src-tauri/fixtures/`.

## Verification

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
# → Passed (no diff)

cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
# → Finished (0 errors, 0 warnings)

cargo test --manifest-path src-tauri/Cargo.toml --workspace
# → test result: ok. 121 passed; 0 failed; 1 ignored

CODEQL_THREADS=0 ./scripts/codeql-local.sh
# → CodeQL local gate green
```

## Deliberate decisions

- Dropped `npm` checks from `.github/workflows/contracts.yml` because the `src/` directory and `package.json` no longer exist.
- Allowed removing dead code `set_snippet_editing` from `lists_ui.rs` to satisfy the strict `-D warnings` constraint instead of blindly adding `#[allow(dead_code)]`.
