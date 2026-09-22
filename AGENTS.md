# Agent notes · Soufflé

Conventions for agents working in this repo live in [`CLAUDE.md`](./CLAUDE.md).

**Slint UI:** the shipped frontend is native Slint (`src-tauri/souffle-slint/`), not Svelte. The senior checklist (closed-set enums, house widgets, Metal/Skia, tab keep-alive, memory, threads) is [`docs/engineering/slint.md`](./docs/engineering/slint.md). Follow it for any `.slint` or `souffle-slint` change.

**CodeQL:** not a PR check. Run `./scripts/codeql-local.sh` before opening or updating a PR (requires `brew install --cask codeql`). GitHub only runs CodeQL on push to `develop` (see `.github/workflows/codeql.yml`).

## Engineering rules

One language, one app. Rust owns the truth. The shipped UI is Slint. In both cases a mistake must become a failed build instead of a wrong pixel six weeks later. The Slint half of these rules, plus performance and memory, is spelled out in [`docs/engineering/slint.md`](./docs/engineering/slint.md).

### 1. One declaration, in the contract

Every value belonging to a **closed set** is declared exactly once.

**Shipped UI (Slint):** `export enum` in `src-tauri/souffle-slint/ui/types.slint`, matching `souffle_lib` enum, converted with an exhaustive `match` both ways (`settings_ui.rs`, `ia_ui.rs`, …). Never a string property or a string callback argument. Type-design rules (enum vs catalogue, no sentinels, no speculative traits) are in [`docs/engineering/slint.md`](./docs/engineering/slint.md), adapted from Microsoft's Framework Design Guidelines.


Closed set means: the set of valid values is decided by the code, not by the user or by data.
Recording states, permission states, export formats, transport kinds, error reasons, search
source kinds. If adding a value requires editing Rust, it is a closed set.

- Never write the same string literal on both sides of the Slint/Rust boundary
  IPC). If a conditional compares against a string, that string is an enum variant somewhere.

**Open sets are different and must not be converted.** Engine, model and backend ids, user-editable
template ids, device UIDs: their values come from data or from the user, so the backend ships a
catalogue and the frontend reads it. `TranscriptionCatalog` is the reference implementation of that
pattern. Copying a list of options into a `.slint` file is the anti-pattern, whichever kind of set
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

**Slint**

- Branch on the `types.slint` enum, never on a parallel string. Slint has no exhaustiveness
  check: adding a variant is a review reject until every chip / `if phase ==` site is updated,
  and the Rust `match` at the boundary must already fail to compile.

### 3. Reference implementations in this repo

Read these before inventing a shape:

| Pattern | Where |
|---|---|
| Closed-set Slint enums + exhaustive Rust `match` | `ui/types.slint`, `settings_ui.rs` |
| Theme as a reactive global | `ui/theme.slint` |
| Tab keep-alive (no remount) | `ui/components/settings/settings_tabs.slint` |
| Metal renderer pin | `souffle-slint/Cargo.toml` (`renderer-skia`), `main.rs` (`renderer_name("skia")`) |
| Catalogue for an open set | `TranscriptionCatalog`, `src-tauri/src/engine/mod.rs` |

Full Slint checklist: [`docs/engineering/slint.md`](./docs/engineering/slint.md).

### 4. What a review rejects

- A string literal compared in a conditional that is also declared elsewhere.
- A `_ =>` or a `default:` added to a domain enum or a contract union.
- An enum used for an open set, a reserved/sentinel variant, `_ =>` "for forward compat", or a public `trait` with one implementor and no consumer.
- A tagged union flattened in `souffle_lib` (`mode` + leftover `Option`s) instead of an ADT. The Slint Window may split payload; the lib type must not.
- A list of options, bounds or defaults copied from Rust into a `.slint` file.
- A generated file edited by hand.
- A frequent view switch (`if tab ==`) that destroys and recreates the item tree.
- FemtoVG left as the renderer (missing `renderer-skia` or `renderer_name("skia")`).
- Palette hex or Fluent `std-widgets` chrome where a house widget + `Theme.*` exists.
- A user-facing Slint string outside `@tr()`. For a label computed in Rust (which `@tr()` can't
  wrap, since it only accepts a compile-time literal), route it through a sentinel-token switch
  like `ThemedComboBox`'s — and the token emitted by Rust must match the literal the Slint switch
  checks for exactly, both call sites (header label and dropdown-list item) updated together. A
  mismatch that silently falls through to the raw token is the same class of bug as skipping
  `@tr()` entirely, not a smaller one.
- A new Python or Node script added to `scripts/` (or anywhere else) for something a Rust
  binary/test or a POSIX shell script calling existing Rust tooling could do instead. This repo is
  one language, on purpose (SOU-185: no Node/npm in the shipped build) — a scripting-language
  dependency introduced as a shortcut is a regression against that goal, not a neutral convenience,
  and ships with a ticket to replace it if it can't be avoided outright.
