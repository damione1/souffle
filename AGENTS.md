# Souffle Coding Standards

This document is the repo-level implementation standard for Souffle. It complements [CLAUDE.md](CLAUDE.md) and is intended to reduce do/undo cleanup across iterations.

## Architecture

- Keep the current split: Tauri command layer -> domain/service logic -> storage/runtime primitives.
- Tauri commands stay thin. They validate inputs, call a focused backend service/helper, and return `Result<T, String>`.
- Do not move inference onto Tokio. Keep the existing threading model: UI/runtime on Tokio, audio on `std::thread`, inference on `std::thread`.
- Runtime lifecycle belongs in backend runtime/pipeline modules, not in UI components.
- Persisted settings stay behind a typed settings contract. SQLite key/value storage is an implementation detail, not the app-facing API.
- Keep transcription concepts separated:
  engine/family, model, backend runtime, and downloadable artifact are different layers and should not be collapsed into one enum or one string id.

## Rust Placement Rules

- Enums and DTOs that cross the frontend/backend boundary belong in focused shared modules, not inline inside command files.
- Engine-facing types live under `src-tauri/src/engine/`.
- Engine catalogs, active transcription profiles, and runtime status DTOs belong in `src-tauri/src/engine/mod.rs` or adjacent engine modules.
- Artifact download/storage logic belongs under `src-tauri/src/models/`, driven by typed artifact descriptors from the engine registry.
- Persisted meeting and transcript DTOs live in `src-tauri/src/transcript.rs`.
- App settings and shortcut DTOs live in `src-tauri/src/settings.rs`.
- Raw SQLite access stays in `src-tauri/src/db/`.
- Command orchestration stays in `src-tauri/src/commands/`.
- Reusable runtime/lifecycle logic belongs in dedicated modules such as `pipeline/`, `state.rs`, or a service module, not duplicated across commands.

## Rust Coding Rules

- No `unwrap()` or `expect()` in production paths.
- Prefer `thiserror` domain errors internally and convert to `String` once at the Tauri boundary.
- Add comments only for non-obvious design decisions or safety/lifecycle constraints.
- New settings, DTOs, or command payloads must be typed and serializable with Serde.
- Avoid free-form JSON maps for internal app contracts when a typed struct is practical.
- If a resource has a lifecycle, make shutdown/idempotency explicit.
- New STT engines or model families must register through descriptor/profile DTOs before any UI or command-layer wiring.
- Runtime implementations must be created from a resolved profile (`engine + model + backend`), not from a single engine id.
- Metadata such as supported languages, streaming support, and memory guidance belongs in descriptors, not on the runtime trait itself.
- Download sources must be expressed as typed artifact descriptors, not hard-coded repo matches scattered across commands.
- Provider model lists must be exposed as typed descriptors, not raw `Vec<String>` payloads.

## Frontend Placement Rules

- Shared TypeScript types that mirror Rust DTOs belong in `src/lib/types/`.
- IPC wrappers belong in `src/lib/api/`.
- Bootstrapping and cross-view initialization logic belong in dedicated modules such as `src/lib/bootstrap.ts`, not duplicated in view components.
- Feature-specific orchestration belongs in `src/lib/features/<feature>/controller.svelte.ts`.
- Large views should be composition shells that render feature section components from `src/lib/features/<feature>/components/`.
- Shared feature selection helpers belong in `src/lib/features/<feature>/` when multiple views/controllers need the same DTO lookup logic.
- Shared UI primitives belong in `src/lib/components/ui/`.
- Large view components may own presentation, but not ad hoc persistence and parsing logic. Move backend I/O helpers and normalization into API/bootstrap modules first.

## Frontend Coding Rules

- Keep Svelte view files focused on rendering and local interaction state.
- Do not hand-parse arbitrary backend JSON in components.
- Use `@lucide/svelte` for app icons in Svelte 5 components. Do not add inline SVG markup unless the icon is custom artwork that does not exist in the shared library.
- Use `errorMessage()` for surfaced errors.
- Keep app store state as the single frontend source of truth for view/recording state.
- Preserve the existing UI language and layout unless a task explicitly requests UX changes.
- Drive engine/model/backend selection UIs from descriptor DTOs, not hard-coded labels or engine-specific assumptions.

## Tests

- Backend tests live next to the Rust module they verify.
- Frontend unit tests live next to the utility or module they verify.
- Every architecture change should add or update tests for the changed contract or lifecycle behavior.
- Minimum verification for cleanup work:
  `cargo test --manifest-path src-tauri/Cargo.toml`
  `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
  `npm test -- --run`
  `npm run check`
  `npm run build`

## Cleanup Bias

- Prefer consolidating implicit behavior into one typed contract over adding another helper around inconsistent code.
- Prefer small, decision-complete modules over giant "misc" utility files.
- Before removing existing patterns, check recent git history to confirm whether they were introduced deliberately.
- Avoid churn-only rewrites. If code moves, it should improve ownership, typing, or lifecycle clarity.
