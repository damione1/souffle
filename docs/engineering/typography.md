# Typography contract

The shipped Slint UI and native macOS HUD share `app/souffle-typography`. This Rust crate owns the Inter assets, their identities, nine weights, slants and role styles. Its build-time projections generate the Slint library and Swift declarations into `OUT_DIR`; generated files are never edited or checked in. `TypefaceWeight` is generated from the Rust enum, re-exported by `types.slint`, and converted exhaustively at the Rust callback boundary. Role names are compile-time global members, never string callback arguments.

## Fonts and resolution

The assets are unmodified static text fonts from the [official Inter 4.1 release](https://github.com/rsms/inter/releases/tag/v4.1). The archive hash and SIL Open Font License are in `app/souffle-typography/assets/README.md` and `LICENSE.txt`. There are eighteen embedded faces: weights 100–900, each upright and true italic. No optical Display fonts, Archivo or JetBrains Mono remain in the active bundle. The bundle also contains the Inter license and provenance under `Contents/Resources/licenses/Inter`.

Build validation requires the expected filename, SHA-256, internal family and PostScript names, static face, OS/2 weight, italic flag and layout glyphs. A missing or modified file aborts compilation by name. The regular and bold faces legitimately omit typographic name ID 16; their family name ID 1 is `Inter`.

At startup Slint's shared Fontique collection must resolve every requested weight/slant to the expected embedded memory source and SHA-256, with no emboldening or skew. Failure aborts before showing the main window. The HUD registers these same Rust bytes in the process before panel creation, retains each `CGFont`, and creates `CTFont`/`NSFont` directly from that retained source. Registration, internal-name or missing-source failures are explicit; no system-font fallback is used. Fonts are never installed globally and require no runtime download. Arbitrary user text outside Inter's character repertoire remains subject to the renderer's ordinary Unicode fallback; the layout contract covers current FR/EN labels and shortcut symbols.

## Components and enforcement

Use `AppText { typography: Typography.section; }` for text, `AppTextInput` for editable text, and `AppNumericText` for durations/timestamps. The default role is `Typography.body`. Main Window defaults reference that same role. House inputs forward `root.typography`, including their placeholders; they never copy style fields. Conditional roles select only named contract members.

The build runs a comment/string-aware Rust validator across every authored `.slint` source, using Slint's underscore/hyphen identifier equivalence. It rejects raw text items, local `font-*`/letter-spacing definitions or assignments, literal/copy style objects, two-way role bindings, field overrides, contract mutation, uncontracted Window defaults, legacy font names and UI-owned font assets, with file and line. Both sides of two-way bindings are checked: a local alias cannot expose an inherited font property, a typography input or a generated role for mutation. Ordinary text/value bindings and metric reads remain valid. Native HUD labels must be constructed by the generated `Typography.makeHUDLabel()` / `makeHUDLiveText()` factories, which bind their exact roles at creation. The native validator rejects raw AppKit text controls, font reassignment and constructor or multiline font modifications. Negative isolated fixtures run in the workspace gate. Adding a style requires editing the Rust table; adding a weight breaks the exhaustive callback conversion.

Slint 1.18 has no public OpenType feature switch used here. `AppNumericText` reserves a separate fixed-width slot for each digit using the maximum digit advance of the exact Inter face. Punctuation keeps its measured advance. This preserves digit cell positions at rollover without relying on proportional digits or claiming `tnum` support. One accessible label exposes the complete counter; decorative per-glyph children have no accessible label. Existing duration formatting and recording timing remain unchanged.

## Inventory

The first SOU-276 baseline at `e0371545` recorded 318 font/size/weight/spacing source references, including comments, five bundled font files and four native system-font/fallback paths. During implementation `develop` advanced to `0004eced` (SOU-166), adding four font-size references and five text items. The final baseline inventory is therefore 322 references and 229 text items; the table below compares that merged base with the final contract migration. Counts include editable inputs; text-free components contribute no text surface. Text roles also cover dropdown headers/items, dialog content, placeholders, virtualized transcript words, summary Markdown, log rows, empty/loading/error messages and onboarding states through their shared house components. SOU-166's paragraph editor, learning-mode enum, suggestion callbacks, virtualization and tab keep-alive remain intact.

| Active UI file | Before text items | After contract items | Selected roles |
| --- | ---: | ---: | --- |
| `components/action_hero.slint` | 6 | 6 | Typography.action-title, Typography.caption, Typography.hint |
| `components/app_header.slint` | 3 | 3 | Typography.brand, Typography.caption |
| `components/audio_player_section.slint` | 2 | 2 | Typography.caption, Typography.secondary |
| `components/dictation_recovery.slint` | 2 | 2 | Typography.body-emphasis, Typography.secondary |
| `components/empty_state.slint` | 2 | 2 | Typography.body-emphasis |
| `components/filter_chip.slint` | 1 | 1 | Typography.hud-label |
| `components/live_paragraph_editor.slint` | 1 | 1 | Typography.body |
| `components/modal_dialog.slint` | 2 | 2 | Typography.body, Typography.dialog-strong |
| `components/onboarding/markdown_view.slint` | 5 | 5 | Typography.field-strong, Typography.secondary, Typography.small |
| `components/onboarding/permissions_step.slint` | 7 | 7 | Typography.caption, Typography.caption-label, Typography.hint-emphasis, Typography.micro-detail |
| `components/onboarding/update_available_dialog.slint` | 2 | 2 | Typography.secondary |
| `components/search_field.slint` | 2 | 2 | Typography.field-label |
| `components/settings/about_section.slint` | 4 | 4 | Typography.content |
| `components/settings/calendar_section.slint` | 5 | 5 | Typography.field-label, Typography.secondary, Typography.table-heading |
| `components/settings/data_section.slint` | 11 | 11 | Typography.caption, Typography.field-label, Typography.secondary, Typography.small |
| `components/settings/diagnostics_section.slint` | 4 | 4 | Typography.field-label, Typography.micro-detail, Typography.secondary |
| `components/settings/dictation_polish_section.slint` | 1 | 1 | Typography.small |
| `components/settings/dictionary_section.slint` | 13 | 13 | Typography.caption, Typography.hint, Typography.micro-detail, Typography.secondary, Typography.table-heading |
| `components/settings/feedback_sounds_section.slint` | 1 | 1 | Typography.secondary |
| `components/settings/intelligence_section.slint` | 6 | 6 | Typography.caption, Typography.small |
| `components/settings/interface_section.slint` | 4 | 4 | Typography.label, Typography.secondary |
| `components/settings/microphone_section.slint` | 9 | 9 | Typography.caption, Typography.content, Typography.field-label, Typography.hint, Typography.secondary, Typography.small |
| `components/settings/model_delete_section.slint` | 1 | 1 | Typography.small |
| `components/settings/model_section.slint` | 2 | 2 | Typography.caption, Typography.small |
| `components/settings/settings_field.slint` | 2 | 2 | Typography.field-label, Typography.secondary |
| `components/settings/settings_field_vertical.slint` | 2 | 2 | Typography.field-label, Typography.secondary |
| `components/settings/settings_group.slint` | 1 | 1 | Typography.eyebrow |
| `components/settings/settings_permissions_dialog.slint` | 1 | 1 | Typography.secondary |
| `components/settings/settings_tabs.slint` | 1 | 1 | root.active ? Typography.section : Typography.label |
| `components/settings/snippets_section.slint` | 11 | 11 | Typography.caption, Typography.hint-emphasis, Typography.secondary, Typography.table-heading |
| `components/settings/status_indicator.slint` | 1 | 1 | Typography.secondary |
| `components/settings/summary_templates_section.slint` | 1 | 1 | Typography.secondary |
| `components/status_banner.slint` | 1 | 1 | Typography.content |
| `components/summary_section.slint` | 23 | 23 | Typography.body-emphasis, Typography.caption, Typography.caption-emphasis, Typography.hint, Typography.micro-detail, Typography.secondary, Typography.section, Typography.small |
| `components/themed_button.slint` | 1 | 1 | root.compact ? (root.kind == ThemedButtonKind.default ? Typography.small-label : Typography.small-emphasis) : (root.kind == ThemedButtonKind.default ? Typography.label : Typography.section) |
| `components/themed_checkbox.slint` | 1 | 1 | Typography.caption-strong |
| `components/themed_combo.slint` | 2 | 2 | Typography.body, Typography.content |
| `components/themed_line_edit.slint` | 2 | 2 | Typography.body, root.typography |
| `components/themed_spin.slint` | 1 | 1 | Typography.body |
| `components/themed_text_area.slint` | 2 | 2 | Typography.body |
| `components/timeline_item.slint` | 5 | 5 | Typography.field-label, Typography.hint, Typography.secondary-emphasis, Typography.small |
| `components/timeline_section.slint` | 10 | 10 | Typography.date-heading, Typography.field-label, Typography.secondary, Typography.secondary-emphasis, Typography.small, Typography.timeline-heading |
| `components/transcript_section.slint` | 6 | 6 | Typography.caption-emphasis, Typography.micro, Typography.micro-detail, Typography.secondary, Typography.section, Typography.small |
| `components/transcript_words.slint` | 4 | 4 | Typography.content, Typography.section, root.typography |
| `idle_view.slint` | 1 | 1 | Typography.body |
| `main_window.slint` | 0 | 0 |  |
| `meeting_detail.slint` | 21 | 21 | Typography.caption, Typography.content, Typography.hint, Typography.secondary, Typography.section, Typography.small, Typography.title, Typography.title-input |
| `onboarding_view.slint` | 16 | 16 | Typography.body, Typography.caption-emphasis, Typography.content, Typography.dialog-title, Typography.secondary, Typography.title |
| `recording_view.slint` | 13 | 13 | Typography.action-body, Typography.body, Typography.caption-emphasis, Typography.content, Typography.field-heading, Typography.hint, Typography.hint-emphasis, Typography.micro-detail, Typography.preview, Typography.small, Typography.small-emphasis |
| `settings_view.slint` | 4 | 4 | Typography.content, Typography.settings-title |

The native HUD has two text controls: mode label (`hud-label`) and live transcript (`hud-live`). Its wrapping and width measurement both use `hud-live`; there are no additional native font sources. Its retained five-line tail uses the same Inter ascender/descender/leading metrics for allocation. Dictation has compact and expanded forms; meeting uses its existing icon-only compact form, and polishing uses its existing compact label. All three modes consume the same contract.

## Reproducing proofs

`cargo run --manifest-path app/Cargo.toml -p souffle-typography --example font_inventory` regenerates the asset and role tables below. `cargo test --manifest-path app/Cargo.toml -p souffle-typography` verifies assets, current FR/EN layout glyphs, role guards and digit slot advances. The Slint typography tests exercise real Fontique resolution, missing variants and invalid registration. The live-view counter test verifies rendered text-item positions and sizes for the digit cells and the full accessible label across rollover; it does not inspect glyph origins inside each text item. Existing live-view tests cover tail scrolling, scroll-back and transcript interactions.

`cargo run --manifest-path app/Cargo.toml -p souffle-slint --example typography_qa` displays the compiled shipped components with isolated data and the actual native HUD. It requires Skia/Metal and prints resolved sources for all eighteen faces. It does not bootstrap services or access the user database, recording, network, permissions or preferences. Select surfaces with `SOU276_SURFACE` (documented in the example), `SOU276_LANG=fr|en`, `SOU276_DARK=0|1`, and `SOU276_WIDTH=680|1040`. `SOU276_FAIL_REGISTRATION=1` is fixture-only failure injection and verifies native rejection before layout. `specimen` displays every weight/slant with accents, ampersands, ligatures, digits and shortcuts. HUD verification checks actual CoreText glyph runs and retained tail/descender bounds. Captures must identify the built binary, theme, locale and dimensions; generated CSS/font declarations alone are not rendering evidence.

| File | Family / subfamily | PostScript | Weight | Italic | SHA-256 |
| --- | --- | --- | ---: | --- | --- |
| `Inter-Thin.ttf` | Inter / Thin | `Inter-Thin` | 100 | false | `22453f995345e5618d539315a8501f4c74ca41d1898ce44e7d1b8206f0d097d4` |
| `Inter-ThinItalic.ttf` | Inter / Thin Italic | `Inter-ThinItalic` | 100 | true | `d89ffb2849ef91246283de40a929f671b6e0c8ad1c235701f9362fb0406b4cce` |
| `Inter-ExtraLight.ttf` | Inter / ExtraLight | `Inter-ExtraLight` | 200 | false | `f3c7079d9eb4a799251a844acea9eb55f99094479306e2ac1bee875dd9f7ae80` |
| `Inter-ExtraLightItalic.ttf` | Inter / ExtraLight Italic | `Inter-ExtraLightItalic` | 200 | true | `22fca887d9a9336e8a29601b19683947a57b53911c74e251c3657ee798635275` |
| `Inter-Light.ttf` | Inter / Light | `Inter-Light` | 300 | false | `164414f0aacbe98a7e64addc43f7b3bfd2e32f7b90e101feeab227f14c371bda` |
| `Inter-LightItalic.ttf` | Inter / Light Italic | `Inter-LightItalic` | 300 | true | `c3f9efa776957eefaeac8a2991a990fd1bba6cb928dbaeab7abd0655f3a7693c` |
| `Inter-Regular.ttf` | Inter / Regular | `Inter-Regular` | 400 | false | `40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82` |
| `Inter-Italic.ttf` | Inter / Italic | `Inter-Italic` | 400 | true | `bbc051dd204b5019a1aa0bc0ae2aa8a05ab13e7a3f979fa357631dc7feb6833a` |
| `Inter-Medium.ttf` | Inter / Medium | `Inter-Medium` | 500 | false | `97ad806f526e41546d46365bb3a393145f75b7b1568913db74549ad8b8dba872` |
| `Inter-MediumItalic.ttf` | Inter / Medium Italic | `Inter-MediumItalic` | 500 | true | `51c2c8d7c36f7c26e6e2678b5c3069b329bde9a081154553b0f5bc2d4fc14075` |
| `Inter-SemiBold.ttf` | Inter / SemiBold | `Inter-SemiBold` | 600 | false | `78a843fade9d4612a5567302fb595b56976eb5fcebf4fea5a5912d638bafcde3` |
| `Inter-SemiBoldItalic.ttf` | Inter / SemiBold Italic | `Inter-SemiBoldItalic` | 600 | true | `eff2930c3d3b35d3fcf5f76252b6baef4c3e907d9d2fde1d16cf5d417f8deef4` |
| `Inter-Bold.ttf` | Inter / Bold | `Inter-Bold` | 700 | false | `288316099b1e0a47a4716d159098005eef7c0066921f34e3200393dbdb01947f` |
| `Inter-BoldItalic.ttf` | Inter / Bold Italic | `Inter-BoldItalic` | 700 | true | `948405a16cdc62701da5f4005ed068ca5f4d27061d98f7974ccfc37831d9581d` |
| `Inter-ExtraBold.ttf` | Inter / ExtraBold | `Inter-ExtraBold` | 800 | false | `e6756ad5690b77606aa62249a7b420d9902d45cae4b0048a24911fd4324b0a22` |
| `Inter-ExtraBoldItalic.ttf` | Inter / ExtraBold Italic | `Inter-ExtraBoldItalic` | 800 | true | `31b58a00cb9e8d00cb936057285282310e81d2f834461264c543c96983af64a6` |
| `Inter-Black.ttf` | Inter / Black | `Inter-Black` | 900 | false | `6342d3ea6dc088b43867f615e807d898adf100c93edb978b8e52c5eb71a264da` |
| `Inter-BlackItalic.ttf` | Inter / Black Italic | `Inter-BlackItalic` | 900 | true | `1737f7d5b391520e9adcc3ea8c730a0854fe6fdd9b9c93c4100c89beb215a931` |

| Role | Size (px / pt) | Weight | Letter spacing |
| --- | ---: | ---: | ---: |
| `body` | 14 | 400 | 0 |
| `hud-live` | 13 | 400 | 0 |
| `hud-label` | 12 | 500 | 0 |
| `micro` | 10 | 400 | 0 |
| `micro-detail` | 10.5 | 400 | 0 |
| `caption` | 11 | 400 | 0 |
| `caption-label` | 11 | 500 | 0 |
| `caption-emphasis` | 11 | 600 | 0 |
| `caption-strong` | 11 | 700 | 0 |
| `small` | 11.5 | 400 | 0 |
| `small-label` | 11.5 | 500 | 0 |
| `small-emphasis` | 11.5 | 600 | 0 |
| `secondary` | 12 | 400 | 0 |
| `secondary-emphasis` | 12 | 600 | 0 |
| `hint` | 12.5 | 400 | 0 |
| `hint-emphasis` | 12.5 | 600 | 0 |
| `content` | 13 | 400 | 0 |
| `label` | 13 | 500 | 0 |
| `section` | 13 | 600 | 0 |
| `field-label` | 13.5 | 400 | 0 |
| `field-heading` | 13.5 | 600 | 0 |
| `field-strong` | 13.5 | 700 | 0 |
| `body-emphasis` | 14 | 600 | 0 |
| `brand` | 14.5 | 600 | 0 |
| `action-title` | 15 | 600 | 0 |
| `action-body` | 15 | 400 | 0 |
| `dialog-title` | 18 | 600 | 0 |
| `dialog-strong` | 18 | 700 | 0 |
| `preview` | 19 | 400 | 0 |
| `settings-title` | 20 | 700 | 0 |
| `title-input` | 22 | 400 | 0 |
| `title` | 24 | 700 | 0 |
| `table-heading` | 10.5 | 700 | 1 |
| `date-heading` | 10.5 | 600 | 0.6 |
| `timeline-heading` | 11 | 600 | 1.1 |
| `eyebrow` | 11 | 600 | 1.65 |
