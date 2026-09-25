# Slint component build-vs-reuse audit

Snapshot for SOU-259. This is a decision record, not a component catalogue that must be kept in
sync forever.

- Audited commit: `5a5b268c2c419b3a3ebfba6d7bfcfd5eca7215b3`
- Shipped UI dependency: Slint 1.18.1 (`app/Cargo.lock`), with Skia/wgpu/Metal
- Scope: every visual `component` declaration under `app/souffle-slint/ui/components/`
- Denominator: 57 components in 51 `.slint` files (54 exported, 3 private)
- Excluded: exported structs/enums and row models; they do not render UI

Reproduce the denominator with:

```sh
rg -n '^\s*(export )?component\s+' app/souffle-slint/ui/components --glob '*.slint'
```

## What the available alternatives actually provide

Slint 1.18.1 ships `Button`, `CheckBox`, `ComboBox`, `LineEdit`, `Slider`, `SpinBox`, `Switch`,
`TextEdit`, `TabWidget`, `ListView`, `ScrollView`, `StandardListView`, `Dialog`, and the native
`PopupWindow`, `Path`, `Image`, `Flickable`, and layout primitives. The pinned implementations are
under `i-slint-compiler-1.18.1/widgets/` in Cargo's registry; the corresponding upstream source is
the [Slint 1.18.1 widget tree](https://github.com/slint-ui/slint/tree/v1.18.1/internal/compiler/widgets).

The standard widgets do not provide an application-defined skin in this release. `Palette` exposes
style-selected brushes as read-only properties; only `color-scheme` is writable. Selecting Fluent,
Cupertino, Material, Cosmic, Qt, or native is supported, but mapping those controls to Soufflé's
`Theme` tokens is not. A standard control is therefore a real replacement only when its own chrome
is acceptable, or when it sits below a house wrapper without leaking that chrome.

`ListView` is a special case: in 1.18.1 the compiler marks a `for` repeater that is its direct
child, and the runtime creates only visible or partially visible delegates, including
variable-height delegates. Reading `widgets/common/listview.slint` alone misses that compiler and
runtime path. Nesting the repeater loses the optimization. Supplying `content-height` explicitly
does not disable virtualization, but it does disable the runtime's automatic content-extent
calculation.

The [official examples](https://github.com/slint-ui/slint/tree/v1.18.1/examples) provide patterns,
not a drop-in waveform, transcript word-flow, settings form, or audio player. Dynamic Markdown is
available through Rust's `slint::StyledText::from_markdown`; it supports inline styling, lists and
links, but rejects headings, so release-note headings still need a small block wrapper. Registry
searches on 2026-09-25 found no dedicated Slint waveform widget.
General third-party sets were early and visually unrelated (`slint-ui-system` 0.5.0,
`slint-ui-templates` 0.1.0, both without license metadata on crates.io); adopting one would replace
one house design system with another rather than remove the ownership cost. `lucide-slint`, already
in the build, remains the suitable icon source.

Verdicts:

- **Keep**: feature composition or a house control whose standard alternative cannot preserve its
  contract or theme.
- **Thin wrapper (already)**: the component is useful API/chrome around an existing primitive or
  crate and does not reimplement that primitive.
- **Thin wrapper target**: the component API remains useful, but its implementation should delegate
  more of the behavior to the named standard API in the linked follow-up.
- **Replace**: an existing house primitive covers the same contract with less duplicated behavior;
  implementation belongs in the linked follow-up, not this audit.

## Decision table

| # | Component | Location | Role and evaluated alternative | Verdict |
|---:|---|---|---|---|
| 1 | `ActionHero` | `components/action_hero.slint:6` | Product-specific pair of recording CTAs; standard buttons do not provide the combined layout, state, and labels. | **Keep** — feature composition. |
| 2 | `AppHeader` | `components/app_header.slint:22` | Custom macOS overlay header, brand/status pill, theme and Settings actions; no standard widget owns this window-chrome contract. | **Keep** — native-window integration and product identity. |
| 3 | `AudioPlayerSection` | `components/audio_player_section.slint:8` | Audio transport plus seek waveform. The old repeated bars are already two native `Path` items after SOU-258; plotter crates/examples would add a texture pipeline for no gain. | **Keep** — already uses the native drawing primitive. |
| 4 | `DictationRecovery` | `components/dictation_recovery.slint:9` | Product recovery message/actions composed from `ThemedButton`; not a generic input widget. | **Keep** — feature composition. |
| 5 | `EmptyState` | `components/empty_state.slint:6` | Reusable icon/title/body layout; no stateful primitive is being reimplemented. | **Keep** — small presentational composition. |
| 6 | `FilterChip` | `components/filter_chip.slint:17` | Controlled single-select chip. A checkable standard `Button` owns independent checked state and does not model the shared enum source of truth. | **Keep** — controlled filter semantics and house chrome. |
| 7 | `LucideIcon` | `components/lucide_icon.slint:9` | Maps `Theme` stroke/fill defaults onto `lucide-slint`'s `IconDisplay`. | **Thin wrapper (already)** — typed crate asset, no copied SVG. |
| 8 | `ModalDialog` | `components/modal_dialog.slint:15` | In-window scrim/panel/Escape/click-outside shell. Built-in `Dialog` is a top-level window with platform button layout, not an equivalent overlay. | **Keep** — shared product modal chrome. |
| 9 | `MarkdownView` | `components/onboarding/markdown_view.slint:6` | Renders Rust-parsed dynamic release notes. Although DSL `@markdown()` is literal-only, Rust 1.18.1 exposes `StyledText::from_markdown`; it preserves inline emphasis/lists/links and only needs a small heading wrapper. | **Thin wrapper target → SOU-266** — replace the lossy inline parser, preserve heading layout. |
| 10 | `ReleaseNotesBox` | `components/onboarding/markdown_view.slint:55` | Border/padding around a stock `ScrollView` containing `MarkdownView`. | **Thin wrapper (already)** — standard scrolling, house container only. |
| 11 | `PermissionsStep` | `components/onboarding/permissions_step.slint:15` | Product permission rows, typed access states, actions, and onboarding copy. | **Keep** — feature composition. |
| 12 | `UpdateAvailableDialog` | `components/onboarding/update_available_dialog.slint:24` | Typed updater phases and actions inside shared `ModalDialog`; standard `Dialog` has no updater state machine. | **Keep** — feature specialization. |
| 13 | `WhatsNewDialog` | `components/onboarding/whats_new_dialog.slint:11` | Dynamic release notes and acknowledgement inside shared `ModalDialog`. | **Keep** — feature specialization. |
| 14 | `SearchField` | `components/search_field.slint:8` | Compact icon-leading search field. Standard `LineEdit` cannot expose its internal padding/colors to `Theme` in 1.18.1. | **Keep** — search-specific API and house chrome. |
| 15 | `AboutSection` | `components/settings/about_section.slint:13` | Settings content and actions for version/update/support. | **Keep** — feature section, not a widget primitive. |
| 16 | `AudioSection` | `components/settings/audio_section.slint:12` | Settings composition for capture/retention choices. | **Keep** — feature section using shared controls. |
| 17 | `AutostartSection` | `components/settings/autostart_section.slint:6` | Settings row binding the app's autostart contract. | **Keep** — feature section using shared controls. |
| 18 | `CalendarSection` | `components/settings/calendar_section.slint:19` | Calendar permission/list composition with product row model. | **Keep** — feature section; no calendar-picker equivalent is needed here. |
| 19 | `CodeBlock` | `components/settings/data_section.slint:8` | Selectable, read-only themed text in a `Flickable`. Standard `TextEdit` adds Fluent chrome that cannot be restyled and no editing behavior is required. | **Keep** — small private presentation with native text selection. |
| 20 | `DataSection` | `components/settings/data_section.slint:52` | Export/import/data-location settings and code-block composition. | **Keep** — feature section. |
| 21 | `DiagnosticsSection` | `components/settings/diagnostics_section.slint:15` | Diagnostics output/actions are feature composition. Its live log manually slices a `Flickable`, while a direct `ListView` repeater is natively virtualized. | **Keep section; replace list core → SOU-267.** |
| 22 | `DictationPolishSection` | `components/settings/dictation_polish_section.slint:14` | Product toggle/provider availability composition. | **Keep** — feature section. |
| 23 | `DictionarySection` | `components/settings/dictionary_section.slint:21` | CRUD form is feature composition. The Rust slice/spacers were added from the false premise that `ListView` mounts all rows; 1.18.1 virtualizes a direct repeater and measures variable delegates. | **Keep section; replace list core → SOU-267.** |
| 24 | `FeedbackSoundsSection` | `components/settings/feedback_sounds_section.slint:8` | Sound toggle and volume setting composition. | **Keep** — feature section. |
| 25 | `IntelligenceSection` | `components/settings/intelligence_section.slint:17` | Provider catalogue/status/error actions. | **Keep** — feature section over an open Rust catalogue. |
| 26 | `ShortcutRecorderButton` | `components/settings/interface_section.slint:18` | Focused key-capture state machine shared by Settings and onboarding. Standard `Button` does not record arbitrary shortcuts. | **Keep** — unique interaction contract. |
| 27 | `InterfaceSection` | `components/settings/interface_section.slint:84` | Theme, locale, shortcut, paste and HUD settings composition. | **Keep** — feature section. |
| 28 | `MicIconButton` | `components/settings/microphone_section.slint:11` | Private button interaction duplicates `ThemedButton`; only the typed Lucide glyph, square geometry, and glyph color differ. | **Replace → SOU-265** — shared house icon-button primitive; medium risk, Settings › Microphone only, four call sites. |
| 29 | `MicrophoneSection` | `components/settings/microphone_section.slint:85` | Device catalogue, priority ordering, disconnected/hidden states and actions. | **Keep** — feature section over Rust-precomputed rows. |
| 30 | `ModelDeleteSection` | `components/settings/model_delete_section.slint:20` | Destructive confirmation flow for a model. | **Keep** — feature composition using `ThemedButton`. |
| 31 | `ModelSection` | `components/settings/model_section.slint:13` | Model catalogue/status selection. | **Keep** — feature section over an open Rust catalogue. |
| 32 | `PermissionsSection` | `components/settings/permissions_section.slint:6` | Settings entry point into the shared permission dialog. | **Keep** — feature section. |
| 33 | `SettingsField` | `components/settings/settings_field.slint:7` | Label/description/divider plus horizontal child slot. Standard layout boxes do not encode the app's form-row contract. | **Keep** — house form-layout primitive. |
| 34 | `SettingsFieldVertical` | `components/settings/settings_field_vertical.slint:3` | Vertical variant of the app's form-row contract. | **Keep** — house form-layout primitive. |
| 35 | `SettingsGroup` | `components/settings/settings_group.slint:6` | Group title/rule/child slot. Standard `GroupBox` uses selected-style chrome rather than this flat Settings hierarchy. | **Keep** — house form-layout primitive. |
| 36 | `SettingsPermissionsDialog` | `components/settings/settings_permissions_dialog.slint:10` | Permission guidance/content inside shared `ModalDialog`. | **Keep** — feature specialization. |
| 37 | `SettingsTabChip` | `components/settings/settings_tabs.slint:4` | Private controlled tab with typed navigation and AX role. Standard tab buttons cannot be independently themed. | **Keep** — controlled house tab chrome. |
| 38 | `SettingsTabBar` | `components/settings/settings_tabs.slint:66` | Exhaustive `SettingsTab` selection and compact non-stretching tabs. Standard `TabWidget` also keeps its static tabs mounted, but uses selected-style chrome and owns a different bar/content API. | **Keep** — typed navigation and product layout. |
| 39 | `SettingsTabPage` | `components/settings/settings_tabs.slint:172` | Keeps per-tab scrolling under the separately themed `SettingsTabBar`. Standard `TabWidget` keeps static tab trees too, but adopting it would merge bar/content ownership and reintroduce style-owned chrome. | **Keep** — per-tab scroll and house-tab composition, not a unique keep-alive capability. |
| 40 | `SnippetsSection` | `components/settings/snippets_section.slint:19` | CRUD/edit forms are feature composition; its manual Rust slice/spacers duplicate native direct-repeater `ListView` virtualization. | **Keep section; replace list core → SOU-267.** |
| 41 | `StatusIndicator` | `components/settings/status_indicator.slint:6` | Compact colored dot plus label aligned in Settings rows. | **Keep** — tiny presentational primitive. |
| 42 | `SummaryTemplatesSection` | `components/settings/summary_templates_section.slint:15` | Template catalogue/editor composition. | **Keep** — feature section over Rust-owned data. |
| 43 | `StatusBanner` | `components/status_banner.slint:12` | Product warning message with optional action and dismissal. Standard dialogs/toasts do not match its inline lifetime or house chrome. | **Keep** — shared feedback composition. |
| 44 | `SummarySection` | `components/summary_section.slint:15` | Summary generation, model selection, copy feedback and structured result sections. | **Keep** — feature composition; local button cleanup is separate from replacing the section. |
| 45 | `ThemedButton` | `components/themed_button.slint:14` | Button semantics plus five product variants. Standard `Button` colors come from a read-only style palette, not `Theme`. | **Keep** — canonical house chrome/control contract. |
| 46 | `ThemedCheckbox` | `components/themed_checkbox.slint:4` | Compact 16 px calendar selector. Standard `CheckBox` has selected-style size/color and includes label layout. | **Keep** — deliberate compact control. |
| 47 | `ThemedComboBox` | `components/themed_combo.slint:7` | Themed `PopupWindow`, keyboard/AX, stable open-set ids, selected index, and translation-token display. Standard `ComboBox` exposes strings/index only and cannot take `Theme`. | **Keep** — broader boundary contract than the standard widget. |
| 48 | `ThemedLineEdit` | `components/themed_line_edit.slint:4` | Single-line input with house height/colors, focus-lost callback, font sizing and controlled text. Standard `LineEdit` internals are not themeable in 1.18.1. | **Keep** — boundary and visual contract. |
| 49 | `ThemedSlider` | `components/themed_slider.slint:5` | Integer slider with house track/focus/AX. Standard `Slider` has stronger base behavior but its visible track/thumb cannot be mapped to `Theme`; an invisible-control overlay would be speculative and duplicate hit geometry. | **Keep** — revisit when Slint exposes public control bases or writable style tokens. |
| 50 | `ThemedSpinBox` | `components/themed_spin.slint:6` | Integer display/stepper with product sizing and callbacks. Standard `SpinBox` is also integer-valued, but its internals remain style-owned and not themeable. | **Keep** — house sizing/theme, not numeric type. |
| 51 | `ThemedSwitch` | `components/themed_switch.slint:6` | Compact themed boolean control. Standard `Switch` colors/geometry belong to the selected style. | **Keep** — product chrome with typed bool state. |
| 52 | `ThemedTextArea` | `components/themed_text_area.slint:6` | Multiline controlled input with house chrome. Standard `TextEdit` has useful menus but its border, padding, selection and focus chrome are not externally themeable. | **Keep** — visual contract outweighs the unavailable base reuse. |
| 53 | `TimelineItemRow` | `components/timeline_item.slint:19` | Meeting/dictation row, metadata, hover actions and callbacks. `StandardListViewItem` cannot represent this product row. | **Keep** — feature row delegate. |
| 54 | `TimelineSection` | `components/timeline_section.slint:30` | Upcoming events, calendar setup and grouped timeline are feature composition, but all history rows currently mount under `Flickable`. A flattened direct `ListView` model can preserve the single scroll and virtualize them. | **Keep section; replace list core → SOU-268.** |
| 55 | `TranscriptSection` | `components/transcript_section.slint:54` | Header/copy, timestamps, always-mounted word flow per visible paragraph and alias popup are feature behavior. The manual Rust slice/spacers can be replaced by variable-height direct `ListView` delegates. | **Keep section; replace list core → SOU-269**, retaining the SOU-258 baseline. |
| 56 | `TranscriptWordFlow` | `components/transcript_words.slint:27` | Clickable words in native `FlexboxLayout`; there is no standard rich text widget with per-word callbacks. | **Keep** — already delegates wrapping to the native layout primitive. |
| 57 | `DictionaryAliasPopover` | `components/transcript_words.slint:66` | Zero-size typed anchor around native `PopupWindow`; the anchor exists because Slint forbids enclosing access to popup internals. | **Thin wrapper (already)** — native popup lifecycle, house content only. |

Component-level count: 52 **Keep**, 3 **Thin wrapper (already)**, 1 **Thin wrapper target**, and 1
**Replace**. Five kept feature sections also contain list machinery to replace in three coherent
follow-ups: Settings lists (SOU-267), timeline (SOU-268), and transcript (SOU-269). This audit
intentionally changes no component implementation.

## Decision for new components

Before adding a house component, search the pinned Slint widget sources, built-in elements,
official examples, and crates.io. Record the evaluated alternative and why it fails the required
theme, typed boundary, interaction, accessibility, or measured performance contract. Prefer a thin
house wrapper over a native primitive or maintained crate. A component that merely composes a
feature screen is not evidence that a generic widget was reimplemented.

For unbounded lists, verify the complete compiler/runtime behavior, not only the declarative
`ListView` wrapper source. Keep the model repeater as a direct child. Let `ListView` calculate
content height unless the model supplies a correct extent for a measured exception.
