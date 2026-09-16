# Spike Report: standalone Slint shell without Tauri (SOU-186)

Supersedes the report previously embedded in the SOU-186 ticket, which was produced without
independent verification and contained claims that did not hold up under an actual build (see
"QA du stack PR 244-250" in SOU-185, 2026-09-16). Every number and behavior below was reproduced
in this session (2026-09-16) by actually building and driving the app; the exact commands are in
"Verification" at the end.

## Where the code lives

`src-tauri/souffle-slint/` — a **separate Cargo workspace member** (added to
`src-tauri/Cargo.toml`'s `[workspace] members`), with its own `Cargo.toml` and `build.rs`. It does
not depend on the `souffle` package, does not invoke `tauri_build::build()`, and does not touch
`scripts/build-mcp-sidecar.sh`. `cargo build -p souffle-slint` builds it in isolation - verified by
running that exact command with no prior Tauri build in this tree (AC9).

## Rust <-> Slint state/action pattern (AC5)

No specta, no JSON, no generated TypeScript. State the UI needs is a Slint `property`; an action
the UI requests is a Slint `callback`. Both compile to direct Rust function calls via the
`slint::include_modules!()`-generated struct.

```slint
// ui/main_window.slint
in-out property <string> status-text: "idle";
callback ping-requested();
```

```rust
// src/main.rs
let weak = window.as_weak();
window.on_ping_requested(move || {
    ping_count.set(ping_count.get() + 1);
    if let Some(window) = weak.upgrade() {
        window.set_status_text(format!("pinged {} time(s)", ping_count.get()).into());
    }
});
```

Verified end to end, not just compiled: clicking the "Ping backend" button (via a real
accessibility-driven click, see Verification) increments Rust-owned state and the displayed text
updates to `status: pinged 1 time(s)`, then `2`, then `3` across three separate clicks. This is the
pattern [[SOU-187]] onward should reuse for `AppState` exposure in place of specta commands.

## Menu bar (AC2)

A `MenuBar { Menu { title: "Edit"; MenuItem { ... } } }` block inside the `Window` produces a
**native macOS menu bar** for free via Slint's winit backend (`muda` underneath) - no AppKit
bridge code needed. This answers SOU-186's open question #1.

- App menu (About/Hide/Quit) is generated automatically, no code required.
- `Cmd+Q` exits cleanly: verified the process disappears from `ps` after triggering Quit via the
  actual menu item (not just a window close).
- The Edit menu (Cut/Copy/Paste/Select All) had to be declared explicitly; it does **not** appear
  by default. Once declared, wiring `MenuItem.activated` to a Slint widget's own `cut()`/`copy()`/
  `paste()`/`select-all()` functions makes it genuinely functional - verified by driving the actual
  menu items via AppleScript/System Events and checking `pbpaste` and the field's accessible value
  after each action (Select All -> Copy -> clipboard matched field text; Paste -> field value
  changed to the pasted string; Select All -> Cut -> clipboard got the text and field went empty).

Cut/Copy/Paste only work this way against a **`LineEdit`** (or another std-widget), not a bare
`TextInput` - see Accessibility below for why that distinction also matters for a11y.

## Dock / activation policy (AC3)

The app appears in the Dock's list of running applications by default (`NSApplicationActivationPolicyRegular`
is winit's default when a window is shown) - verified via System Events querying the Dock
process's UI element list while the app was running. Cmd-Tab cycling specifically was not
separately exercised in this session.

## Accessibility decision (AC7)

**Accepted, with a mandatory widget-discipline policy** - not the unconditional "100% accessible"
claim of the previous (unverified) report.

Verified finding: Slint's accessibility exposure is **opt-in per element**, not automatic.
- A bare `Text` / `TextInput` primitive has no default `accessible-role` and is **invisible** to
  the system accessibility tree (confirmed: `System Events` could see only the native title bar
  buttons and window title, nothing from a window built with bare primitives).
- A **std-widget** (`LineEdit`, `Button`, ...) sets `accessible-role` explicitly in its own
  implementation (e.g. `accessible-role: text-input` in `LineEdit`, `accessible-role: button` in
  `Button`) and *is* exposed correctly - confirmed: after switching to `LineEdit`/`Button`, the
  same query enumerated the text field, its value, and the button by name.

Decision: acceptable to proceed, **provided** every future screen ([[SOU-187]]-[[SOU-190]]) is
built from `std-widgets` components (or explicitly sets `accessible-role` on any bespoke element)
rather than bare primitives. This is a real constraint the current Svelte/HTML UI doesn't have -
semantic HTML elements (`<input>`, `<button>`) get roles for free. A future PR should add an
automated check (screenshot-sidecar-style accessibility-tree assertion, not manual QA) that fails
if a screen's interactive elements aren't enumerable via `System Events`/AXAPI, so this doesn't
regress silently the way the previous stack's claims did.

## Measurements (AC6)

All measured on this shell only (no backend/audio/ML code linked in yet - that lands with
[[SOU-187]]+, and will move these numbers, especially RSS and compile time, substantially).

| Metric | This shell (2026-09-16) | Previous (unverified) report |
|---|---|---|
| Bundle size | **9.4 MB** (`du -sh` on the produced `.app`) | ~9.4 MB claimed - matches |
| RSS idle (~50s) | **~86.6 MB** (88704 KB via `ps -o rss`, stable from ~2s to ~50s after launch) | ~89 MB claimed - close, plausible after all |
| Compile, cold (release, full dep tree) | **52.05s** (`time cargo build --release -p souffle-slint` from a clean target) | not comparable - previous report gave no cold-release number |
| Compile, warm (release, crate only) | **28.06s** | not given |
| Compile, warm (debug, crate only) | **0.83s** | ~2s claimed - actually better |

Not compared against the full Tauri app's numbers here (that comparison needs a real screen with
real backend wiring, i.e. after [[SOU-187]]); this table is the "coquille" baseline the ticket
asked for.

## AC status

| AC | Status | Evidence |
|---|---|---|
| AC1 | Met | `cargo build -p souffle-slint` succeeds; window renders (screenshot, this session) |
| AC2 | Met | Quit exits cleanly (process check); Edit menu Cut/Copy/Paste/Select All driven via System Events with clipboard/field-value assertions |
| AC3 | Met (Dock); Cmd-Tab not separately tested | Dock process-list query while running |
| AC4 | Met | `scripts/bundle-slint-app.sh` produces a `.app` with no `tauri-bundler`/`cargo-bundle`; launched via `open` (Finder-equivalent) |
| AC5 | Met | `ping-requested`/`status-text` round-trip, driven via a real button click, verified via accessibility value read-back |
| AC6 | Met | Table above, each number reproducible via the commands in Verification |
| AC7 | Met | Decision above, with the concrete opt-in-per-widget finding as its reasoning |
| AC8 | N/A | No structural blocker found - event loop, packaging, and a11y all resolved without a bridge |
| AC9 | Met | `cargo tree -p souffle-slint` has no `tauri*` crate; crate builds standalone |
| AC10 | Met | This report + session screenshots, dated 2026-09-16 |
| AC11 | Met | No existing test file touched by this ticket's changes |
| AC12 | Pending | Requires a human (Damien) to personally reproduce the manual checks below before merge |

## Verification

```bash
cargo build --manifest-path src-tauri/Cargo.toml -p souffle-slint
cargo tree --manifest-path src-tauri/Cargo.toml -p souffle-slint   # no tauri* crate
./scripts/bundle-slint-app.sh                                        # produces the .app, prints its size
```

Manual (reproduce before merge, per AC12):
- Launch `src-tauri/target/release/bundle/macos/Soufflé Slint.app` by double-clicking in Finder
  (no terminal open) - confirms AC4's "launches without a terminal" requirement.
- ⌘Q quits without leaving a zombie process.
- Type in the field, use Edit menu Select All / Copy / Cut / Paste, confirm behavior against the
  system clipboard.
- Click "Ping backend" a few times, confirm the status text increments.
- Cmd-Tab into and out of the app, confirm it behaves like a normal foreground app.
