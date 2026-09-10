# Soufflé

Private, on-device speech-to-text for macOS (Apple Silicon only). Tauri 2 app: Rust backend in `src-tauri/`, Svelte 5 frontend in `src/`.

## Commands

- `npm test` runs the frontend unit tests (Vitest)
- `npm run check` type-checks Svelte and TypeScript
- `npm run build` builds the frontend
- `make nightly` builds the debug app as **Soufflé Nightly** (`com.souffle.desktop.nightly`) and opens it; `make nightly-fresh` wipes Nightly data and TCC first; `make nightly-dmg` wraps it in a `.dmg`. Does not touch the installed Soufflé.
- `cargo test --manifest-path src-tauri/Cargo.toml --workspace` runs backend tests
- `npm run generate:types` regenerates `src/lib/types/generated.ts` and `src/lib/api/generated.ts` from the Rust commands; run it after changing any `#[tauri::command]` signature
- `./scripts/codeql-local.sh` runs CodeQL (rust, javascript-typescript, actions) locally; required before opening or updating a PR. Needs `brew install --cask codeql`. CI CodeQL runs only on push to `develop`, not on PRs.

## Local gate (before every PR open / push)

Mirror `.github/workflows/contracts.yml`, then CodeQL (not on PR CI):

```bash
npm ci   # fresh worktree only
./scripts/build-mcp-sidecar.test.sh && ./scripts/build-mcp-sidecar.sh
npm run check:generated-types
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --workspace
npm test
npm run check
./scripts/codeql-local.sh
```

## Release changelogs

Every release tag must carry hand-written notes in the annotated tag message: `git tag -a v0.6.0` and write the notes in the editor. The release workflow uses that message as the GitHub release body; a tag without a message falls back to auto-generated notes from merged PR titles, which is a safety net, not the goal.

Structure the notes as:

- One-line hook summarizing the release for a user
- Themed sections (`## Fixed`, `## Improved`, `## New`) with user-facing bullets: what changed and why it matters, no internal codenames, no commit-speak
- Explicit callouts for breaking changes, and for known limitations a user will actually hit
- No test or QA section. Steps to reproduce, checklists, things to validate and anything addressed to a reviewer stay in the PR description; a release note only tells a user what changed in the app

The in-app What's New dialog renders these notes as Markdown, so headings, bold, lists, and `https://github.com/...` links are welcome. Keep the language plain: the reader is someone dictating notes, not the person who wrote the diff.
