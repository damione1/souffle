# Agent notes · Soufflé

Conventions for agents working in this repo live in [`CLAUDE.md`](./CLAUDE.md).

**CodeQL:** not a PR check. Run `./scripts/codeql-local.sh` before opening or updating a PR (requires `brew install --cask codeql`). GitHub only runs CodeQL on push to `develop` (see `.github/workflows/codeql.yml`).

## Engineering rules

Two languages, one app. Rust owns the truth, TypeScript renders it, and the only thing keeping
them honest is the generated contract. These rules exist so that a mistake becomes a failed
build instead of a wrong pixel six weeks later.

### 1. One declaration, in the contract

Every value belonging to a **closed set** is declared exactly once, as a Rust enum carrying
`#[derive(specta::Type)]`, and reaches the frontend through the generated client
(`src/lib/types/generated.ts`). The frontend imports it. It never retypes it.

Closed set means: the set of valid values is decided by the code, not by the user or by data.
Recording states, permission states, export formats, transport kinds, error reasons, search
source kinds. If adding a value requires editing Rust, it is a closed set.

- Never write the same string literal on both sides of the IPC. If a conditional compares against
  a string, that string is an enum variant somewhere, and the comparison must import it.
- Never hand-write `impl specta::Type` to flatten an enum into `String`. That silently deletes the
  contract for that type and forces the frontend to invent it again.
- After changing any `#[tauri::command]` signature or any exposed type, run
  `npm run generate:types` and commit both generated files. CI fails otherwise.

**Open sets are different and must not be converted.** Engine, model and backend ids, user-editable
template ids, device UIDs: their values come from data or from the user, so the backend ships a
catalogue and the frontend reads it. `TranscriptionCatalog` is the reference implementation of that
pattern. Copying a list of options into a `.svelte` file is the anti-pattern, whichever kind of set
it is.

### 2. Exhaustive branching, enforced by the compiler

A single declaration is necessary and not sufficient. `kind === "me" ? "Me" : "Them"` still
compiles the day a third variant appears, and quietly files the new value under `Them`. That is
the failure this rule exists to prevent.

We work in compiled languages on purpose. When a change makes existing code wrong, the compiler
must say so, by name and by line. Any construct that suppresses that signal is a bug in the making,
even when today's behaviour is correct.

**Rust**

- Never `_ =>` on a domain enum. Name every variant. Adding a variant must break the build at
  every site that has to decide something about it.
- `_ =>` stays legitimate for genuinely open domains: FFI integers, `char`, `Result`, tuples,
  raw CoreAudio codes.
- Prefer `match` over `matches!` plus `else` when the result depends on which variant it is.
  `matches!` collapses an enum to a boolean and takes the exhaustiveness check with it.

**TypeScript**

- Read a contract union with a `switch` that has **no** `default`, or with a
  `Record<Union, T>`. Both make an added variant a compile error.
- Never a binary ternary on a contract union. Never a `default` that absorbs the unknown case.
- When a fallback genuinely is the semantics (an `"auto"` value that means "all of them"), keep it,
  and make the residue explicit with `assertNever` so an unplanned variant still fails.
- Type a lookup table on the generated union, not on itself. `Record<MeetingTranscriptionLanguage,
  string>` fails when Rust gains a language; `Record<(typeof options)[number], string>` never does,
  and the option silently disappears from the UI.

### 3. Reference implementations in this repo

Read these before inventing a shape:

| Pattern | Where |
|---|---|
| Exhaustive `Record` over a generated union | `src/lib/features/meeting/system-audio.ts` |
| Exhaustive `switch`, no `default` | `src/lib/stores/app.svelte.ts` (`deriveRuntimePhase`) |
| Typed registry, impossible to mistype | `src/lib/features/settings/anchors.ts` |
| Catalogue for an open set | `TranscriptionCatalog`, `src-tauri/src/engine/mod.rs` |
| Typed events and commands | `src-tauri/src/app_events.rs`, `specta_builder()` in `src-tauri/src/lib.rs` |

### 4. What a review rejects

- A string literal compared in a conditional that is also declared elsewhere.
- A `_ =>` or a `default:` added to a domain enum or a contract union.
- A list of options, bounds or defaults copied from Rust into a `.svelte` or `.ts` file.
- A generated file edited by hand.
