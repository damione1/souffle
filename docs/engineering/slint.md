# Slint development guide

Senior checklist for the shipped Soufflé UI. The binary crate is `souffle-slint` (`app/souffle-slint/`). Rust owns the truth. Slint renders it. Nothing in the shipped app reads the retired Svelte tree under `src/`.

Read this before adding a `.slint` file, a Slint enum, a house widget, or a `VecModel` push. The
[SOU-259 component audit](slint-components.md) records the current build-vs-reuse decisions.
Official Slint docs this guide compresses: [best practices](https://docs.slint.dev/latest/docs/slint/guide/development/best-practices/), [reactivity](https://docs.slint.dev/latest/docs/slint/guide/language/concepts/reactivity/), [properties](https://docs.slint.dev/latest/docs/slint/guide/language/coding/properties/), [globals](https://docs.slint.dev/latest/docs/slint/guide/language/coding/globals/), [functions and callbacks](https://docs.slint.dev/latest/docs/slint/guide/language/coding/functions-and-callbacks/), [repetition and models](https://docs.slint.dev/latest/docs/slint/guide/language/coding/repetition-and-data-models/), [backends and renderers](https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backends_and_renderers/), [debugging](https://docs.slint.dev/latest/docs/slint/guide/development/debugging_techniques/), [ListView](https://docs.slint.dev/latest/docs/slint/reference/std-widgets/views/listview/).

Type-design rules (enums, traits, abstractions) are adapted from Microsoft's [Framework Design Guidelines](https://learn.microsoft.com/en-us/dotnet/standard/design-guidelines/type) and checked against the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/). The CLR words are class, struct, interface. Here they are enum, struct, trait, Slint `export enum`. The obsession is the same: the type is the contract. FDG is not a line-by-line port; the deltas below are the parts that would go wrong if you copied C# into Rust.

## 1. Contracts: one declaration, exhaustive at every site

The same rule as the rest of the repo, applied to Slint instead of TypeScript.

A **closed set** is a set whose valid values are decided by the code, not by the user or by data: recording mode, settings tab, theme, paste method, log level, permission state. If adding a value requires editing Rust or a `.slint` enum, it is a closed set.

### Declare it once

1. Domain enum in `souffle_lib` (the source of truth for persistence and engine).
2. Matching `export enum` in [`ui/types.slint`](../../app/souffle-slint/ui/types.slint).
3. Exhaustive `match` both ways at the boundary (`settings_ui.rs`, `ia_ui.rs`, …). No string property. No string callback argument. No `to_string()` round-trip.

```
souffle_lib::settings::Theme  <-->  types.slint AppTheme  via  theme_to_slint / theme_from_slint
```

Never write the same string literal on both sides of the Slint/Rust boundary. If a conditional compares against a string, that string is an enum variant somewhere.

Open sets (engine ids, model ids, device UIDs, user-editable template ids) stay catalogues. Rust ships the list. Slint reads a `VecModel`. Copying the list into a `.slint` file is the anti-pattern, for both kinds of set.

Bounds and defaults live in `SettingsOptions::current()`, not as magic numbers in a `.slint` file.

### Type design: pick the right kind of type

[FDG](https://learn.microsoft.com/en-us/dotnet/standard/design-guidelines/type): each type is a well-defined set of related members, not a random bag of functionality. `settings_ui.rs` converts settings. It does not also play audio.

| FDG | Here |
|---|---|
| Simple enum | Rust enum + Slint `export enum`. Exclusive closed set. |
| Flags enum | Almost never. Only if every bitwise combination is valid. |
| Interface / abstract class | `trait`. Only when several implementors **and** at least one consumer already exist. |
| Class (identity) | Owned struct with a lifecycle (`MainWindow`, `AudioPlayer`). |
| Struct (value) | Small immutable struct or `Copy` enum. A color token, a duration, `PasteMethod`. |
| Static constants of related values | An enum. Not `const THEME_DARK: &str = "dark"`. |

A pair of bools that cannot both be true is an enum you have not named yet. Three screens gated by `settings-open` plus `recording-mode` is `RecordingMode` plus a separate overlay flag, not four booleans.

Do **not** import from FDG: the 16-byte class-vs-struct heuristic, the `I` prefix, abstract classes as a versioning hatch, static classes as buckets of methods (that is a module), or “adding an enum value is a small compat risk”. Those are CLR rules. Ownership, modules, and exhaustive `match` replace them.

### Enums are for small closed sets

From [Enum Design](https://learn.microsoft.com/en-us/dotnet/standard/design-guidelines/enum):

- Strongly type every parameter, property, and return value that represents a set of values. `in property <AppTheme> theme`, never `in property <string> theme`.
- Prefer an enum to static constants, string literals, or boolean pairs.
- Do not use an enum for an **open** set (OS version, friend names, model ids, device UIDs). That is a catalogue Rust ships and Slint reads as a `VecModel`.
- Do not reserve dummy variants "for later". Add the variant when it exists. Reserved values pollute the real set and get matched by accident.
- Do not ship an enum with a single variant. That is a bool you were afraid to name, or a set that is not closed yet.
- Do not put **sentinel** values in the enum. A sentinel tracks state *about* the set (`Unspecified`, `All` stuffed into the item kind) rather than being a member of it. `TimelineKind` is `{ meeting, dictation }`. The filter that can also mean "all" is a **different type**: `TimelineFilter`. `ShortcutField.none` is a real choice (no field is capturing). `AccessState.unknown` is a real observed permission, not a placeholder.
- Simple enums are exclusive. `AudioRetention` is one of off / 7d / 30d / forever, never a combination. A flags enum (plural name, powers of two) only when every combination is valid. Invalid combinations (`keep-7d | keep-forever`) mean it was never flags.
- Adding a variant after ship is expected **and is a breaking change** for every exhaustive `match` in this crate and downstream. That is the point. FDG treats the risk as small because C# `switch` is not exhaustive. Do not add `_ =>` “for forward compat”. Do not `#[non_exhaustive]` on a domain enum that the UI must handle completely.
- Absence is `Option`. Failure is `Result`. Neither belongs as a fake variant (`Missing`, `Error`) on a domain enum, and neither is a magic zero. `Default` on an enum only if that variant is the real product default (`AppTheme::System` might be; `LogLevel` probably is not).
- Open-but-distinct values (device UID, model id, template id) are a **newtype** (`DeviceUid(String)`) plus a catalogue, not a 400-variant enum and not a raw `String` that leaks into every signature.
- Rust enums carry data. Prefer `enum Mode { Meeting { id: MeetingId }, Dictation, Idle }` in `souffle_lib` over `mode: String` plus `meeting_id: Option<String>`. Slint enums are C-like (no payload): at the UI boundary the extra fields sit on the Window (`recording-mode` + `active-meeting-id`) and **one** Rust site keeps the pair consistent. Do not flatten the lib type to match Slint.

### Naming types and variants

From [Names of classes, structs, and interfaces](https://learn.microsoft.com/en-us/dotnet/standard/design-guidelines/names-of-classes-structs-and-interfaces):

- Simple enum: singular (`AppTheme`, `SettingsTab`, `PasteMethod`).
- Flags enum: plural. We almost never have one.
- No `Enum` / `Flags` suffix (`ThemeEnum`).
- No prefix on variants (`Dark`, not `ThemeDark`). Slint already writes `AppTheme.dark`.
- Types are nouns. A capability trait can be an adjective (`Clone`, `Send`). Rust does not prefix `I`.
- A derived type may carry the base name (`SettingsTabPage`).

### Abstractions: traits, not wishful interfaces

From [Abstractions](https://learn.microsoft.com/en-us/dotnet/standard/design-guidelines/abstractions-abstract-types-and-interfaces) and [Interface Design](https://learn.microsoft.com/en-us/dotnet/standard/design-guidelines/interface):

An abstraction describes a contract without a full implementation. Getting the member set right is the hard part: too many members and nobody can implement it; too few and it is useless in the cases you actually have. Names of abstractions are necessarily vague, so a forest of them makes the crate unreadable unless you hold the whole graph in your head.

- Do not introduce a public `trait` until you have written **several** concrete implementations **and** at least one API that consumes it (`impl Trait`, `&dyn Trait`, or a generic bound). One implementor means it should be a concrete type.
- Exception: a `pub(crate)` (sealed) trait that exists only so this crate can treat a few internal types uniformly. That is not a public abstraction. It still needs more than one impl.
- Prefer a concrete struct. A trait is for a common API across types that do not share a crate-local hierarchy, or for stubbing a heavy dependency in tests. `dyn Trait` only when you need runtime polymorphism; generics are the default.
- Avoid user-defined marker traits with no members. `Send` / `Sync` / `Copy` are language markers. For our types, a newtype or an enum variant is the real model.
- Adding a **required** method to a trait that already has impls is a break. Update every impl in the same PR. A defaulted method is the Rust equivalent of FDG's "add to an abstract class, not to an interface". Do not `#[allow]` a missing impl.
- The Slint equivalent of an interface is the component's `in` / `out` / callback surface. Keep it the smallest set the parent actually uses. A component with twenty `in-out` properties is an abstraction that was never tested against a second consumer.

### Values vs identity

From [Choosing between class and struct](https://learn.microsoft.com/en-us/dotnet/standard/design-guidelines/choosing-between-class-and-struct), mapped to Rust. Ignore the 16-byte / boxing numbers; they are GC folklore.

- Small, immutable, "this is a value" (`PasteMethod`, a duration in minutes): `enum` or `struct`. `Copy` only if a copy has no extra meaning.
- Identity, lifecycle, shared mutation (`MainWindow`, `AudioPlayer`, `VecModel`): owned, often `Rc<RefCell<_>>` on the UI thread.
- Do not `Clone` a large type to dodge the borrow checker.

### Exhaustive branching

A single declaration is necessary and not sufficient. `tab == SettingsTab.ai ? "IA" : "Autre"` still compiles the day a seventh tab appears, and silently files it under "Autre".

**Rust**

- Never `_ =>` on a domain enum. Name every variant. Adding a variant must break the build at every site that has to decide something about it.
- `_ =>` stays legitimate for genuinely open domains: FFI integers, `char`, `Result`, tuples, raw CoreAudio codes.
- Prefer `match` over `matches!` plus `else` when the result depends on which variant it is. `matches!` collapses an enum to a boolean and takes the exhaustiveness check with it.
- Converting a legacy on-disk string (locale stored as `"en"` / `"fr"`) is the exception: name the fallback, log the unknown value, convert into the Slint enum at the boundary. The reverse (`locale_from_slint`) stays exhaustive.

**Slint**

- Branch on the enum (`root.tab == SettingsTab.interface`), never on a parallel string.
- A component-local enum is fine when the set never crosses into Rust (`ThemedButtonKind`). The moment Rust has to know, it moves to `types.slint`.
- Slint has no `match` exhaustiveness check. Compensate: keep the enum in `types.slint`, convert exhaustively in Rust, and when a `.slint` file lists every variant (tab bar chips, onboarding phases), adding a variant is a review reject until every site is updated.

**Retired TypeScript (`src/`, until SOU-192 AC8)**

Same rule, different artefact: `#[derive(specta::Type)]` on the Rust enum, generated client in `src/lib/types/generated.ts`, `switch` with no `default` or `Record<Union, T>`. Never a binary ternary on a contract union. Never hand-write `impl specta::Type` to flatten an enum into `String`. After changing a `#[tauri::command]` or an exposed type, `npm run generate:types` and commit both generated files. Nothing in the shipped app reads this path anymore.

### What a review rejects

- A string literal compared in a conditional that is also declared elsewhere.
- A `_ =>` or a `default:` on a domain enum or a contract union.
- A list of options, bounds, or defaults copied from Rust into a `.slint`, `.svelte`, or `.ts` file.
- A generated file edited by hand.
- A new closed set introduced as `in property <string> kind`.
- An enum used for an open set (ids, names, versions).
- A reserved or sentinel variant (`Future`, `All` inside the item-kind enum, a one-variant enum).
- A tagged union flattened into a discriminant plus leftover `Option` fields in `souffle_lib` (the Slint Window may split them; the lib type must not).
- `_ =>` or `#[non_exhaustive]` on a domain enum "for forward compat".
- `Default` on an enum whose zero/first variant is not the real product default.
- A raw `String` in a signature that is really a typed id (missing newtype).
- A public `trait` with a single implementor and no consumer.
- `ThemeEnum`, `kindDark`, or a pair of bools that is really a three-value enum.

### Reference implementations

| Pattern | Where |
|---|---|
| Closed-set Slint enums | `ui/types.slint` |
| Exhaustive Rust `match` both ways | `settings_ui.rs` (`theme_to_slint` / `theme_from_slint`, …) |
| Open-set catalogue | `TranscriptionCatalog`, `app/src/engine/mod.rs` |
| Bounds from the backend, not the UI | `SettingsOptions::current()` in `settings_ui.rs` |
| Theme as a reactive global | `ui/theme.slint` |
| Tab keep-alive (no remount) | `ui/components/settings/settings_tabs.slint` (`SettingsTabPage`) |
| House widget, not Fluent | `ui/components/themed_button.slint` |
| Modal dialog chrome (scrim, panel, Escape, click-outside) | `ui/components/modal_dialog.slint`, inherited by `whats_new_dialog.slint` / `update_available_dialog.slint` / `settings_permissions_dialog.slint` |
| `Option<Enum>` across the boundary | `update_ui::project_install_block` (a `bool` + enum pair, written by one Rust site) |
| Metal renderer pin | `souffle-slint/Cargo.toml` (`renderer-skia`) + `main.rs` (`renderer_name("skia")`) |

## 2. Layout of the UI crate

Official recommendation: keep business logic, `.slint`, and assets in separate trees. We already do:

```
app/souffle-slint/
├── src/            Rust: callbacks, models, conversions
└── ui/
    ├── main_window.slint    entry Window
    ├── types.slint          closed-set enums
    ├── theme.slint          palette global
    ├── components/          house widgets + feature sections
    └── assets/              fonts, images
```

- One concern per file. A settings section is a component. A reusable control (`ThemedSwitch`, `SettingsField`) is a component. The Window is an assembler, not a dumping ground.
- Import what you use. Do not re-export types from random files; `types.slint` and `theme.slint` are the two globals every file may import.
- `std-widgets.slint` Fluent skins ignore `Theme`. For anything the user sees as chrome (buttons, tabs, switches, combos, fields), use the house widgets under `ui/components/`. Stock `TextEdit` / `ListView` / `ScrollView` stay acceptable when they are not the visual identity.
- Before writing a house component, check the widget APIs in the **pinned** Slint version, native
  elements (`Path`, `Image`, `PopupWindow`, layouts), official examples, and maintained crates.
  State the alternatives evaluated and the rejected contract (theme, typed data, interaction,
  accessibility, or measured performance) in the PR. Prefer a thin wrapper over an existing
  primitive. “The standard widget exists” is not enough when its style cannot consume `Theme`, and
  “this file is custom” is not enough to call a feature-level composition a reimplemented widget.

## 3. Component API: DRY and typed

Treat a component's properties as its public interface. Official qualifiers:

| Qualifier | Who writes | Use for |
|---|---|---|
| `in` | parent / Rust | Inputs. The component must not assign to it. |
| `out` | the component | Derived state the parent reads. |
| `in-out` | both | Two-way state (`<=>` with a parent property, or a tab the bar and the page share). |
| `private` (default) | the component | Internal. Keep it that way. |

- Prefer `in` over `in-out`. `in-out` is a shared mutable cell. Every extra writer is a bug surface.
- Forward callbacks with `callback clicked <=> area.clicked;` rather than wrapping them. Same for properties: `in-out property tab <=> root.tab`.
- Functions live in `.slint` when they are pure view logic (`lengthToInt`). Callbacks live in `.slint` as declarations and in Rust as handlers. Do not put engine I/O, disk, or TCC in a `.slint` function.
- Mark functions and callbacks `pure` when they have no side effects, so bindings can call them. Bindings **must** be pure: evaluating a property must not write another property. The compiler enforces this in pure contexts.
- `init =>` runs after bindings settle. Do not use it to initialize properties. That fights the declarative model. Set defaults on the property declaration.
- Assigning `foo.bar = 42;` from a callback **breaks** the binding on `bar`. After that, `bar` no longer reacts. If the parent still owns the value, emit a callback and let Rust (or the parent) write the source property.

### Theme

`Theme` is a global whose colors are bindings on `Theme.dark`. Rust sets `dark` once at launch and again on every theme change. Every other file reads `Theme.canvas`, `Theme.accent`, … It never hardcodes a palette hex, and it never assumes "gold means primary": in light theme the accent is brown (`#95610f`), success is green, and the Ready chip follows **accent**, not success.

Pin `NSAppearance` to the same resolved value (`appearance::apply_resolved`). Otherwise the native chrome (scrollbars, combo popups) stays on the OS appearance while the in-app palette flips.

### i18n

Official rule: wrap user-visible strings in `@tr("...")` and substitute with `{}`, never `+`. `"Hello, " + name` cannot be reordered by a translator. `@tr("Hello, {}", name)` can. We are not there yet (copy is still hardcoded French). New strings should go in `@tr` so the eventual pass is mechanical.

## 4. Performance

A compiled native app is **not** automatically Metal, and Metal is **not** automatically snappy if the item tree is torn down on every click.

### Renderer

| Renderer | Graphics API on macOS | Default? | Use |
|---|---|---|---|
| FemtoVG | OpenGL ES | Yes, unless we pin otherwise | Do not. Text and path quality is weaker; tab switches and scrolling feel like a webview. |
| Skia | **Metal** | Only if `renderer-skia` is enabled **and** selected | This is what we ship. |
| Software | CPU | Fallback | Debug / MCU. Not the desktop app. |

Pin both sides:

```toml
slint = { version = "1.9", features = ["unstable-winit-030", "renderer-skia"] }
```

```rust
slint::BackendSelector::new()
    .renderer_name("skia".into())
    // ...
    .select()?;
```

`SLINT_BACKEND=winit-skia` is the env-var equivalent. Do not remove `renderer-skia` to shrink the bundle without measuring the UI. Skia costs disk (~15 MB extra here). FemtoVG costs interaction.

`make nightly` is a **debug** bundle. `/Applications/Soufflé.app` is **release -O**. Comparing the two is not a renderer bug.

Since Slint 1.18, Skia on macOS draws through **wgpu → Metal** (`i-slint-renderer-skia` feature `wgpu-30`); there is no separate Skia-Metal surface any more, and a failed wgpu surface silently falls back to Skia's CPU (softbuffer) surface. Verified live on Nightly for SOU-258: `sample <pid>` while scrolling shows `wgpu_hal::metal::surface` / `wgpu_core::present` frames and no `software_surface`. Re-check this way after a Slint upgrade.

### `if` vs keep-alive

`if condition: SomeComponent { }` **destroys and recreates** the whole subtree when `condition` flips. That is the right tool for mutually exclusive screens that are expensive to keep (Idle vs MeetingDetail vs Recording, Onboarding vs the main shell). It is the wrong tool for a tab bar.

Settings tabs used to be `if root.tab == SettingsTab.xxx`. Each click rebuilt hundreds of fields. That was ~400 ms, independent of the GPU.

The keep-alive pattern:

```
SettingsTabPage {
    active: root.tab == SettingsTab.interface;
    InterfaceSection { /* ... */ }
}
```

`SettingsTabPage` sets `visible` and collapses `height` to `0px` when inactive. `visible: false` **still occupies layout space**, so height has to go to zero. `clip: true` stops the collapsed children from painting.

Rule of thumb:

- Rare, heavy, mutually exclusive: `if` (onboarding, recording vs idle).
- Frequent switches, same parent: keep mounted, toggle `active` / `visible` + height.
- First open of a keep-alive group can still hitch (initial create). Subsequent switches must be immediate.

### Lists

- `ListView` instantiates only visible or partially visible rows when the model `for` is its direct
  child. This is a compiler/runtime optimization; reading the declarative `ListView` wrapper alone
  does not reveal it. Letting the widget own `content-height` also enables automatic content-extent
  calculation; an explicit value does not disable virtualization, but it must remain correct. Use
  `ListView` for unbounded lists (timeline, dictionary, transcript). `ScrollView`/`Flickable` plus
  `for`, or a repeater nested below another layout, builds every child. Fine for a handful of
  settings groups, not for history.
- Prefer mutating a `VecModel` (`set_row_data`, `push`, `remove`, `row_changed`) over `set_foo(Rc::new(VecModel::from(everything)))` on every keystroke. Replacing the model rebuilds every row.
- Fixed row height is easier on the virtualizer. Variable-height rows in a hand-rolled `for` will jank.

### Draw calls and `cache-rendering-hint`

Slint re-evaluates only dirty bindings, then paints. Cost is dominated by **how many GPU commands** a frame produces, not by "native vs web".

- Gradients, shadows, and clipped rounded rects often break batching (each rectangle becomes its own draw call under Skia).
- A subtree that rarely changes (a static chart, a decorated header) can be flattened to a texture with `cache-rendering-hint: true`. That trades GPU memory for fewer calls. Do not spray it on every component.
- Plain `Theme.*` fills batch. Prefer them over per-row gradients.

### Bindings

Re-evaluation is lazy: a binding runs when something reads it, and only after a dependency is dirty. Keep expressions cheap. A binding that walks a model on every mouse-move will show up in `SLINT_DEBUG_PERFORMANCE`.

Do not drive layout from `animation-tick()` unless the pixel must move every frame. Idle UI should paint at 0 fps. `refresh_lazy` reports ~2 fps for a blinking caret; if you see 60 fps while nothing is moving, something is dirty-looping.

### Measure before arguing

```bash
SLINT_DEBUG_PERFORMANCE=refresh_lazy,console,overlay make nightly
SLINT_DEBUG_PERFORMANCE=refresh_full_speed,overlay make nightly
SLINT_SLOW_ANIMATIONS=4 make nightly
```

- `refresh_lazy`: are we painting when nothing changed?
- `refresh_full_speed`: is the hot path actually 60 fps?
- `debug("tab", root.tab);` prints to stderr (structs as one-line JSON).

A tab click that takes 400 ms with `refresh_lazy` showing a single spike is tree construction, not fill rate. A tab click that is smooth in `refresh_full_speed` overlay but hitchy in real use is still tree construction.

### SOU-211 perception contract

The product requirement is **Metal end to end** for the shipped macOS
app. Selecting Skia is not enough: a run is admissible only when it proves that
the window surface presented by the compositor is Metal. A missing proof is a
failure, not permission to assume Metal.

#### Input-to-present budgets

Each cell is `p95 / per-sample maximum`. A scenario passes only when both
limits pass; averages and `AfterRendering` timestamps cannot substitute for
them.

| SOU-210 scenario | Start -> presented result | Cold budget | Warm budget |
|---|---|---:|---:|
| `open` | Settings command input -> first complete Settings frame | 180 ms / 250 ms | 100 ms / 150 ms |
| `tab` | tab activation -> destination tab frame | 100 ms / 150 ms | 50 ms / 100 ms |
| `theme` | theme activation -> frame where app content and native chrome use the new resolved theme | 100 ms / 150 ms | 50 ms / 100 ms |
| `menu` | menu option activation -> frame showing the committed selection | 75 ms / 125 ms | 50 ms / 100 ms |
| `number` | keyboard/stepper input -> frame showing the new numeric draft | 50 ms / 75 ms | 33 ms / 50 ms |

The end timestamp is the OS compositor presentation time for the first frame
that contains the requested visual state. Slint `AfterRendering` is explicitly
not that timestamp. Persistence completion remains a separate diagnostic: it
must not delay the optimistic `menu` or `number` presentation, and it does not
move the end of either budget.

Measurement conditions are deliberately reproducible:

- a production-signed `release` bundle, launched outside a debugger from
  `/Applications`, with instrumentation enabled but no compiler or profiler
  running;
- the oldest supported Apple Silicon performance tier: MacBook Air M1, 8 GB,
  internal display at 60 Hz, native resolution, Low Power Mode off, on the
  oldest supported macOS release;
- the isolated synthetic fixture `settings-perf-v1`: defaults plus 8 audio
  devices, 12 AI catalogue rows across 3 provider identities, 250 dictionary
  rows, 250 snippets and 50 calendar rows; provider/network responses are
  deterministic and no personal profile, production database, TCC state or
  credentials are read or changed;
- a cold sample is the first occurrence after a fresh process launch. Collect
  30 cold samples, each from a new process;
- a warm sample follows one discarded priming interaction in the same process.
  Collect 100 warm samples across 10 process launches, restoring the same
  visible state before every sample;
- the run metadata records commit SHA, signature identity, release profile,
  fixture version, machine/OS/display, Skia renderer and independently observed
  Metal surface. Missing metadata or presentation timing invalidates the run.

These numbers are guardrails, not measured baselines. SOU-210's current
`AfterRendering` marker may diagnose pipeline stages, but cannot by itself
satisfy this contract — a full compositor-timing integration remains to be implemented.

#### Non-Metal policy

The signed macOS release fails closed before showing the application window
when the surface is non-Metal or cannot be proven Metal. It must never continue
silently on FemtoVG, OpenGL or software rendering. The failure is explicit and
visible through a native macOS alert that does not depend on the Slint surface;
it names the detected renderer/surface, offers a diagnostic-copy action, then
terminates with a non-zero status after dismissal. There is no degraded product
mode.

The failure path must be injectable at the private startup boundary. Tests pass
a synthetic `non-Metal` or `unverified` probe result before window creation and
assert the same visible failure contract used in release. Injection is confined
to the test/dev launch harness: it must not write a preference, production
database, TCC state or user data, and a production launch must not accept an
environment variable that can silently enable a non-Metal surface.

A startup Metal check already exists (`BackendSelector::require_metal()` in
`main.rs`), predating this policy. It currently fails via a raw `.expect()`
panic — no native alert, no diagnostic-copy action, no controlled exit. Closing
that gap to the contract above is the follow-up implementation work, not net-new
enforcement.

## 5. Memory and threads

Slint is retained-mode. The item tree, font cache, image cache, and GPU textures **are** the working set. Dropping a Window does not reliably unload font and image caches (upstream limitation). Do not open extra Windows to "save memory".

### Handles

- Callbacks and `spawn_local` closures that need the Window take `window.as_weak()`, then `upgrade()` on the UI thread. A strong `MainWindow` inside an async task keeps the tree alive after the user closed it, and is an easy reference cycle with `Rc<RefCell<...>>` state.
- `Rc<RefCell<T>>` is the UI-thread cell. It is not `Send`. Cross-thread work goes through `souffle_lib::async_runtime` (or a channel) and comes back with `slint::invoke_from_event_loop`. `slint::spawn_local` is the UI-thread async executor; use it for `.slint`-adjacent futures, not for engine work.
- Never `borrow_mut()` a `RefCell` across an `.await`.
- A click must repaint first. Anything that can take more than a frame (engine start, audio decode, device open) runs on a worker; the view switches on the click and shows an explicit "starting"/"loading" state until the worker's result lands (SOU-258: a meeting start used to `block_on` the whole engine reset on the main thread, 6 s frozen; opening a meeting decoded its audio on the main thread, 3–24 s).
- Measure item cost in a release build before trading correctness for it. SOU-258 first swapped each transcript paragraph between one wrapped `Text` and the per-word flow on hover: the two break lines differently, so text jumped under the pointer. With the transcript already virtualized (~16 paragraphs mounted), the always-mounted word flow blocks the UI thread ~12 ms on open in release (~90 ms debug) against ~7 ms for the swap, so the flow stays.

### Models and images

- Each `set_*` of a fresh `VecModel` allocates a new backing store and drops the old one. For large, high-frequency lists (live transcript), keep one `Rc<VecModel<_>>` and mutate it.
- Skia may hold decoded image pixels **and** a GPU texture until the cache replaces the first. `colorize` keeps a third copy. Do not load full-size screenshots as `Image` properties if a downscaled asset will do.
- `cache-rendering-hint` is extra GPU memory. Use it where you measured a win.

### What we accept

Skia + Metal uses more RAM and more disk than FemtoVG or the software renderer. That is the cost of a snappy desktop UI. We do not switch renderer to chase Activity Monitor.

## 6. Accessibility

House widgets declare accessibility when they pretend to be a control:

```
accessible-role: button;
accessible-label: root.text;
accessible-enabled: root.enabled;
accessible-action-default => { root.clicked(); }
```

A `Rectangle` plus `TouchArea` with no role is invisible to VoiceOver and to the screenshot skill's AX tree. On macOS, Accessibility Inspector only attaches to a **bundle** (`Soufflé Nightly.app`), not to a raw `cargo run` binary.

## 7. Debugging and fairness

| Question | How |
|---|---|
| Is the renderer actually Skia/Metal? | `renderer_name("skia")` must run before the Window is created. Confirm with a frame capture or by grepping `BackendSelector` in `main.rs`. |
| Debug vs release? | `make nightly` is debug. Fair snap vs `/Applications/Soufflé.app` needs a release Nightly. |
| Apple-to-apple screenshots | One `souffle` process at a time. Skill: `souffle-ui-screenshot`. |
| Layout stretch bugs | `HorizontalLayout { alignment: center; }` on the **main** axis inflates children. Settings fields use `alignment: start`. |
| A child that should fill the rest collapses or vanishes | Any `alignment` other than the default `stretch` gives every child its *preferred* size and ignores `*-stretch`. A `ScrollView` then shrinks to a few lines, and a plain `Rectangle` wrapper (no layout of its own, preferred size 0) hides everything inside it (SOU-256). Drop the alignment on the parent and put the `ScrollView` directly in the layout; don't patch it with `height: 100%` or a wrapper. `live_view.rs` tests show how to assert element geometry. |
| Iterate on the live meeting view | `cargo run --manifest-path app/Cargo.toml -p souffle-slint --example live_transcript_mock` streams a fake Me/Them meeting through the app's own live code path (`MOCK_MODE=dictation` for dictation). |

## 8. PR checklist

Copy into a review. Fail the PR on any unchecked box that the diff touches.

**Contracts**

- [ ] New closed set is an `export enum` in `types.slint` plus a `souffle_lib` enum, not a string property.
- [ ] Boundary conversions are exhaustive `match` both ways. No `_ =>` on the domain type.
- [ ] No option list, bound, or default duplicated in a `.slint` file.
- [ ] Open sets arrive as a model from Rust (catalogue), not a hardcoded array in Slint.
- [ ] Enum is singular, variants are unprefixed (`Dark`, not `ThemeDark` / `ThemeEnum`).
- [ ] No reserved or sentinel variant. `All` / `None` / `Unknown` is either a real member of that set, or a different type (`TimelineFilter` vs `TimelineKind`). Absence is `Option`, failure is `Result`.
- [ ] Domain data that varies by variant is an ADT in `souffle_lib`, not `mode` + leftover `Option`s. Slint may split payload onto Window properties; one Rust site keeps the pair in sync.
- [ ] Open-but-distinct ids are a newtype, not a raw `String` and not an enum of every current value.
- [ ] No `_ =>` / `#[non_exhaustive]` on a UI domain enum. Adding a variant is supposed to break the build.
- [ ] New public `trait` has several impls and at least one consumer in the same PR. `pub(crate)` sealed traits still need more than one impl. Otherwise it is a concrete type.
- [ ] Component `in` / `out` / callback surface is the smallest set the parent uses. No speculative `in-out`.

**Structure**

- [ ] New chrome uses a house widget (or extends one). No new Fluent `std-widgets` Button/TabWidget/CheckBox for themed UI.
- [ ] Colors come from `Theme.*`. No palette hex, no dark-theme gold hardcoded as "primary".
- [ ] Property qualifiers are the tightest that works (`in` before `in-out`).
- [ ] Callbacks that talk to the engine are declared in Slint and implemented in Rust.
- [ ] Bindings are pure. No property write inside a binding.

**Performance**

- [ ] `renderer-skia` feature and `renderer_name("skia")` still present.
- [ ] Frequent view switches (tabs, inner panels) keep the tree mounted. `if` is reserved for rare exclusive screens.
- [ ] Inactive keep-alive panels collapse height (not only `visible: false`).
- [ ] Long lists use `ListView` or an equivalent virtualized `for`, not a `ScrollView` of every row.
- [ ] Model updates mutate `VecModel` when the list is large or hot. No full replace on a single-row edit.
- [ ] `cache-rendering-hint` only on a subtree you measured.

**Memory and threads**

- [ ] Closures capture `Weak` (`as_weak`), not a strong `MainWindow`.
- [ ] UI-thread `Rc<RefCell<_>>` never crosses into `Send` tasks. Return path is `invoke_from_event_loop`.
- [ ] No `borrow_mut()` held across `.await`.

**Accessibility and copy**

- [ ] Custom controls have `accessible-role` and `accessible-label`.
- [ ] New user-visible strings are `@tr("...")` with `{}` substitutions.

**Fairness**

- [ ] Perf claims compared debug Nightly to debug Nightly, or release to release.
- [ ] Visual parity used `souffle-ui-screenshot` (one process, cropped window).
