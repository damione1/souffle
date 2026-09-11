---
name: Soufflé
description: A one-sheet billing poster in four inks, where rank is carried by size and line breaks alone.
colors:
  amber: "#e2a22f"
  ink: "#17100a"
  oxblood: "#6b2710"
  paper: "#fbf3e2"
  ink-quiet-on-amber: "#4a3418"
  ink-quiet-on-paper: "#5c4630"
  rule: "rgba(23, 16, 10, 0.26)"
  rule-faint: "rgba(23, 16, 10, 0.14)"
typography:
  display:
    fontFamily: "Archivo, system-ui, sans-serif"
    fontSize: "clamp(40px, 8.4vw, 132px)"
    fontWeight: 800
    lineHeight: 0.85
    letterSpacing: "-0.035em"
    fontVariation: "'wdth' 68"
  billing-1:
    fontFamily: "Archivo, system-ui, sans-serif"
    fontSize: "clamp(38px, 7.4vw, 102px)"
    fontWeight: 800
    lineHeight: 0.9
    letterSpacing: "-0.03em"
    fontVariation: "'wdth' 72"
  billing-2:
    fontFamily: "Archivo, system-ui, sans-serif"
    fontSize: "clamp(34px, 6.2vw, 86px)"
    fontWeight: 800
    lineHeight: 0.9
    letterSpacing: "-0.03em"
    fontVariation: "'wdth' 72"
  billing-3:
    fontFamily: "Archivo, system-ui, sans-serif"
    fontSize: "clamp(30px, 5.2vw, 72px)"
    fontWeight: 800
    lineHeight: 0.9
    letterSpacing: "-0.03em"
    fontVariation: "'wdth' 72"
  billing-4:
    fontFamily: "Archivo, system-ui, sans-serif"
    fontSize: "clamp(27px, 4.4vw, 60px)"
    fontWeight: 800
    lineHeight: 0.9
    letterSpacing: "-0.03em"
    fontVariation: "'wdth' 72"
  title:
    fontFamily: "Archivo, system-ui, sans-serif"
    fontSize: "clamp(26px, 3.4vw, 46px)"
    fontWeight: 800
    lineHeight: 0.98
    letterSpacing: "-0.025em"
    fontVariation: "'wdth' 78"
  lead:
    fontFamily: "Archivo, system-ui, sans-serif"
    fontSize: "clamp(18px, 1.7vw, 22px)"
    fontWeight: 500
    lineHeight: 1.36
    letterSpacing: "-0.012em"
  body:
    fontFamily: "Archivo, system-ui, sans-serif"
    fontSize: "16px"
    fontWeight: 400
    lineHeight: 1.55
  caption:
    fontFamily: "Archivo, system-ui, sans-serif"
    fontSize: "13.5px"
    fontWeight: 400
    lineHeight: 1.5
  label:
    fontFamily: "Archivo, system-ui, sans-serif"
    fontSize: "11.5px"
    fontWeight: 600
    lineHeight: 1.25
    letterSpacing: "0.1em"
    fontVariation: "'wdth' 92"
  mono:
    fontFamily: "JetBrains Mono, ui-monospace, SFMono-Regular, monospace"
    fontSize: "12.5px"
    fontWeight: 400
    lineHeight: 1.3
rounded:
  none: "0"
spacing:
  gutter: "40px"
  gutter-narrow: "22px"
  list-row: "9px"
  rule-row: "11px"
  act-top: "clamp(52px, 7vw, 104px)"
  act-bottom: "clamp(40px, 5vw, 72px)"
  column-gap: "clamp(28px, 4vw, 64px)"
  plate-gap: "clamp(30px, 4vw, 56px)"
components:
  act-ruled:
    textColor: "{colors.ink}"
    typography: "{typography.title}"
    rounded: "{rounded.none}"
    padding: "0 0 9px"
    width: "100%"
  act-ruled-hover:
    textColor: "{colors.oxblood}"
  act-ruled-quiet:
    textColor: "{colors.ink}"
    rounded: "{rounded.none}"
    padding: "0 0 9px"
    size: "clamp(16px, 1.5vw, 21px)"
  copyline:
    textColor: "{colors.ink}"
    typography: "{typography.mono}"
    rounded: "{rounded.none}"
    padding: "11px 0"
    width: "100%"
  copyline-copied:
    textColor: "{colors.oxblood}"
  spec-chip:
    textColor: "{colors.ink}"
    typography: "{typography.mono}"
    rounded: "{rounded.none}"
    padding: "3px 8px"
  bill-row:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.ink}"
    rounded: "{rounded.none}"
    padding: "clamp(16px, 1.8vw, 24px) 0"
  bill-row-billed:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
    rounded: "{rounded.none}"
    padding: "clamp(16px, 1.8vw, 24px) 18px"
  run-act:
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    rounded: "{rounded.none}"
    padding: "4px 9px"
  run-act-playing:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.amber}"
    rounded: "{rounded.none}"
    padding: "4px 9px"
    size: "15px"
  run-act-demoted:
    textColor: "{colors.ink-quiet-on-amber}"
    rounded: "{rounded.none}"
    padding: "4px 9px"
    size: "10.5px"
  run-word:
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    rounded: "{rounded.none}"
    padding: "4px 0"
  docs-nav-active:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
    rounded: "{rounded.none}"
    padding: "5px 8px"
---

# Design System: Soufflé

## Overview

**Creative North Star: "The Billing"**

The site is one sheet of paper, printed both sides. The front is drenched amber
edge to edge; the back — the dense factual half where the models, the privacy
claims and the requirements live — is the same sheet turned over to paper cream.
A mechanical dot screen marks the turn. Nothing on the page is a card, a panel,
a tile or a container. Sections are bands of the same sheet divided by a
hairline, and hierarchy is carried by size and line breaks alone, the way a
concert bill ranks its acts: the headliner is simply the largest type on the
poster, and no numeral, kicker or eyebrow is ever printed to explain the order.

The world is deliberately the opposite of what this software category ships. It
refuses the centred hero over a radial glow, the alternating zig-zag feature
rows, the trio of icon-tile cards and the gradient CTA box. In their place: a
headline flush-set to the full measure by measurement rather than by a clamp,
acts that descend in billing size down the page, a justified tail block packing
the smallest facts at the foot, and a marginalia run across the very top that
is the sheet's own first line rather than a floating bar — it scrolls away with
the paper.

The only picture on the page is the software actually running. Every image is
live HTML and CSS: five reproductions of the real macOS app, mounted as plates
with a hard ink edge. That is the whole proof strategy, and it is why the sheet
carries no photography, no illustration, no decorative shape and no social
proof of any kind.

**Key Characteristics:**
- Four inks, closed set; every intermediate tone is a halftone screen, never a blend
- One variable grotesk at eight steps, condensing as it grows
- Zero radius, zero shadow, zero glass on the page's own chrome
- Rules are hairlines; state is inversion
- Nothing enters, fades or slides; the one authored motion changes rank, not visibility

## Colors

Four inks and nothing else: a full-strength amber ground, a near-black ink, an
oxblood second ink reserved for contact and for the second speaker, and a paper
cream that is the reverse of the sheet.

### Primary
- **Handbill Amber** (`{colors.amber}`): the ground of the front half of the
  sheet — hero, acts, close. It is a ground, never a fill: no button, chip,
  badge or panel is ever painted amber. Its second job is as the flipped text
  colour inside an inverted element on that ground.

### Secondary
- **Oxblood** (`{colors.oxblood}`): the contact ink. It appears on hover of a
  ruled action, a footer link or a docs link; on the copied state of a command
  line; on inline `code` in the docs; and as the ink of the second speaker in
  the feature prose. It colours type and rules only. It never fills an area and
  never sits behind text.

### Neutral
- **Press Ink** (`{colors.ink}`): all primary text on both grounds, every solid
  fill on the page, and the 1px border that mounts a plate. Also the selection
  highlight and the scrollbar thumb.
- **Paper Cream** (`{colors.paper}`): the ground of the reverse half — models,
  privacy, your data, requirements, footer, and the whole docs page — set by the
  `.invert` scope. Also the text colour inside an inverted element on that
  ground.
- **Quiet Ink** (`{colors.ink-quiet-on-amber}` on amber,
  `{colors.ink-quiet-on-paper}` on paper): secondary and supporting text. Two
  values because the ground changes; the dimmed role is re-declared by `.invert`
  rather than being one value that drifts on one of the two sheets.
- **Rule** (`{colors.rule}`) and **Faint Rule** (`{colors.rule-faint}`): the
  hairlines that divide bands, rows, columns and list items. Faint rule is for
  divisions *inside* a block (list items, key/value rows); full rule is for
  divisions *between* blocks. Both are re-declared under `.invert`.

Every sampled text role measures at least 5.26:1 against its own ground.

### Named Rules

**The Four Inks Rule.** Amber, ink, oxblood, paper. There is no fifth colour and
no gradient anywhere in the page's own chrome. If a tone between two inks is
needed, it is printed as a mechanical dot screen — a repeating radial-gradient
dot on a 9px grid for the band that marks the turn of the sheet, and a 5px dot
band along the baseline behind each speaker's name. A screen is a pattern of one
ink; it is never a blend of two.

**The Inversion Rule.** The active, current or featured state is one move: fill
with ink, flip the type to the ground colour. This is the entire state
vocabulary of the page. It is used for the act in play in the bill, the
default model row, the active docs-sidebar link, the current language, and text
selection. Nothing glows, tints, outlines or lifts to say "selected".

**The Contact-Only Oxblood Rule.** Oxblood is earned by touch or by identity —
hover, the copied confirmation, the other speaker — and by nothing else. The
moment oxblood becomes a fill or a resting decoration, the four-ink system reads
as a three-plus-accent system and the poster becomes a website.

## Typography

**Display Font:** Archivo variable, self-hosted, both axes live (weight 100–900,
width 62%–125%), with a `system-ui, sans-serif` fallback
**Body Font:** Archivo (the same face, relaxed to normal width)
**Label/Mono Font:** JetBrains Mono variable, self-hosted

**Character:** One grotesk does every typographic job on the sheet. The width
axis is the scale's second dimension: type condenses as it grows (`wdth` 68 at
display, 72 at the billing tiers, 78 at title, 100 at body, 92 at the small
caps-style label) and the leading tightens on the same curve (0.85 at display,
0.9 at billing, 1.55 at body). The result is that the largest type reads as
poster lettering rather than as an enlarged paragraph, without a second family
being introduced to fake it.

### Hierarchy
- **Display** (800, `clamp(40px, 8.4vw, 132px)`, 0.85): the hero headline and
  the closing headline. Uppercase, condensed hard, flush-set to the measure.
- **Billing 1–4** (800, `clamp(38px, 7.4vw, 102px)` down to
  `clamp(27px, 4.4vw, 60px)`, 0.9): the act headings. Uppercase. The tier is the
  rank; see The Rank-By-Size Rule.
- **Headline** (800, `clamp(34px, 6.2vw, 86px)`, 0.9): the docs page title.
  Shares billing-2's metrics — the docs page is the same sheet at a smaller
  billing, not a different type system.
- **Title** (800, `clamp(26px, 3.4vw, 46px)`, 0.98): sub-headings inside an act
  — the three-point runs, the requirements blocks, the model names, the docs
  section headings. Uppercase.
- **Lead** (500, `clamp(18px, 1.7vw, 22px)`, 1.36): the one-line sub under each
  headline and the body column of each act. Capped at 62ch by `.measure`.
- **Body** (400, 16px, 1.55): the facts lists and the point paragraphs.
- **Caption** (400, 13.5px, 1.5): plate captions, copy lines, secondary notes.
- **Label** (600, 11.5px, `0.1em`, uppercase, `wdth` 92): the marginalia run,
  the footer, plate identifiers, the docs sidebar group headings. This is the
  smallest voice on the sheet and it is always uppercase.
- **Mono** (400, 12.5px, tabular figures): only where there is a real command,
  file size, spec value or timecode. It is never used for atmosphere.

### Named Rules

**The Flush-Set Rule.** The two display headlines are not sized by a clamp. A
typesetter in `site.js` measures each line at a 100px reference, scales it so it
fills the container's width exactly, then runs one correction pass (hinting at
the final size moves the real width, and without the second pass the line
overhangs the margin every rule below it stops at). The block squares off
instead of ragging wherever a clamp happened to stop. The CSS clamp is the
fallback that shows before the measurement runs and stands if it never does —
it is not the design. The fit re-runs on resize and again on `document.fonts.ready`,
because a width measured against the fallback face is the wrong width.

**The Authored Break Rule.** Headline lines are authored per locale in
`_data/*.json` as `hero.titleLines`, rebroken to comparable lengths so that
flush-setting cannot leave one line towering over its neighbour. French runs
longer than English and gets its own breaks. A headline is never handed to the
browser to wrap.

**The One Face Rule.** Archivo does every job. JetBrains Mono appears only where
there is a real command, size or timecode. A third face on the page's own
surfaces is a system violation.

**The Rank-By-Size Rule.** Acts are not peers, and size is the only thing
carrying the difference. Each act heading takes a permanent billing tier
(`.bill-1` … `.bill-4`) that descends down the sheet, with plateaus where two
acts share a rank. Rank is never printed as a numeral, a kicker, an eyebrow, a
step label or a coloured tag.

## Layout

One measure, 1360px, centred, with a 40px gutter that narrows to 22px below
1080px. Everything on the page lives inside it; nothing is full-bleed except the
band grounds themselves and the screened band that marks the turn of the sheet.

Sections are **bands**: each carries the ground colour, and the division between
two bands is a single hairline on the band's top edge — never a gap of page
background showing through, never a margin, never a shadow. The first band
suppresses its rule.

Inside an act the order is fixed and never alternates: heading, then the facts
in two equal columns (prose left, a rule-separated fact list right), then the
plate at full measure. The hero and the close use an asymmetric pair instead —
a fluid first column against a `minmax(340px, 430px)` action column — so the
download sits to the right of the sub at the same optical weight.

Vertical rhythm is fluid rather than a fixed step scale: acts open at
`clamp(52px, 7vw, 104px)` and close at `clamp(40px, 5vw, 72px)`; the close
section opens wider still (`clamp(64px, 8vw, 128px)`). Inside a block the rhythm
is small and constant: 9px for a fact row, 11px for a key/value or command row,
14–18px for the gap between a block and its rule.

The three-point runs use `grid-template-rows: subgrid` so the three headings and
the three paragraphs align across columns regardless of heading length, with a
`@supports not` fallback to block flow. Columns are divided by a faint hairline
on the left edge, with the first column's border suppressed. There are no icon
tiles.

**Responsive.** Two breakpoints, both reductions rather than redesigns. At
1080px the gutter narrows and every two-column grid collapses to one column; the
point runs swap their left hairlines for top hairlines; the docs sidebar stops
being sticky. At 780px the bill of acts and the language pair leave the top line
and fold into a panel opened by a toggle; command lines are allowed to wrap;
key/value rows stack. No horizontal overflow at 390px or 1440px.

### Named Rules

**The Band Rule.** A section is a band of the sheet, separated from its
neighbour by one hairline. Nothing on this page is a card. If a block needs to
be set apart, it is set apart by a rule, a column division or an inversion — not
by a box with a background, a border radius and an inset.

**The Sheet's Own First Line Rule.** The marginalia run at the top is not a
floating bar. It has no fixed position, no backdrop blur, no shadow and no
compact scrolled state; it is the top line of the sheet and it scrolls away with
the paper.

## Elevation & Depth

This system has no elevation. There is not one shadow in the page's own
stylesheet, and the only `box-shadow` declaration present is a reset that
removes the app mock's own shadow. There is no blur, no glass, no translucency,
no layer that floats above another.

Depth is replaced by ink order. Things are in front because they are darker,
larger, or inverted out of the sheet — the way ink sits on paper rather than the
way panes sit in space. A plate is mounted, not floated: a 1px ink border, hard
edge, flat against the band.

### Named Rules

**The Flat-Sheet Rule.** No `box-shadow` on any element the page owns. If a
surface needs to be distinguished, use a hairline, an inversion, or a screened
band. Shadows belong to the app being pictured, not to the sheet it is printed
on.

**The Plate Rule.** A screen of the app mounts as a plate: `border-radius: 0`,
`box-shadow: none`, `outline: none`, and a 1px ink border — all forced with
`!important` over the mock's own window chrome. A poster does not float a
window, it prints a plate.

## Shapes

Zero radius on everything the page owns, and the rule is enforced rather than
merely observed: the plate reset sets `border-radius: 0 !important`, and the
overlay progress chips re-declare `border-radius: 0` over the inherited value.
There is no pill, no capsule, no rounded tile, no circle.

Borders are hairlines: 1px in `rule` or `rule-faint`, with exactly three
exceptions that are all deliberate emphasis — the 3px rule under a primary ruled
action, the 2px rule under the download word in the marginalia run, and the 1px
solid-ink border of a plate and of the narrow-screen menu toggle.

The only non-rectangular geometry on the page is the dot screen, which is a
repeating radial-gradient dot used at two scales: 1.6px dots on a 9px grid for
the band at the turn of the sheet, 1.15px dots on a 5px grid for the baseline
band under a speaker's name. The band is printed under the baseline rather than
behind the glyph so the word keeps its silhouette.

Progress chips in the overlay stage butt together with a `-1px` left margin so
adjacent borders collapse into one shared hairline, like a printed table.

## Components

### Ruled Actions (the primary action)
- **Character:** an action here is a billing line with a rule under it.
- **Shape:** no radius, no fill, full width; a 3px solid ink rule beneath, 9px
  of breathing room above it.
- **Type:** `clamp(21px, 2.3vw, 32px)`, weight 800, uppercase, `wdth` 74, with
  the label at the left baseline and an inline SVG arrow pushed to the right.
- **Hover:** text and rule both go oxblood over 140ms linear. Nothing moves.
- **Quiet variant:** same line, rule drops to 1px, size to
  `clamp(16px, 1.5vw, 21px)`, width relaxes to 82. Used for the secondary action
  at the close.
- **Never:** a pill, a filled block, a rounded rectangle, a drop shadow, a
  translate on hover.

### Command Lines
- **Character:** the brew command, printed as a line of the sheet with a copy
  affordance at the right margin.
- **Shape:** full-width button, no border, no fill, a hairline along the top
  edge, 11px vertical padding.
- **Type:** the command itself in mono at 12.5px, truncated with an ellipsis
  rather than wrapped; the action label in the small uppercase label voice.
- **States:** hover and copied both turn the right-hand label oxblood; the
  copied state clears itself after 1400ms. No toast, no tooltip, no icon swap.
- **Narrow:** below 780px the line is allowed to wrap and the command breaks
  anywhere rather than truncating.

### The Bill (act navigation, and the signature interaction)
- **Character:** the running order of the page, printed across the top line of
  the sheet as plain words, with the act you are reading taking top billing.
- **Default:** label voice, 4px/9px padding, no fill.
- **In play:** inverted out of the sheet (ink fill, amber type) and stepped up
  to 15px at `wdth` 78 with `0.06em` tracking.
- **Demoted:** every other name in the run gives way — 10.5px, width relaxed to
  100, colour dropped to quiet ink.
- **Played:** acts behind the reader are struck off with a 1px rule across the
  name at 55% opacity, the way a bill marks a set that has already played.
- **Reserved width:** each name is measured once in its *largest* state and
  pinned to that width, so the row never shifts as rank changes. The measurement
  suppresses the transition first, because a width read straight after adding
  the class returns the size the type is animating away from; it re-runs on
  resize and on `document.fonts.ready`.
- **How rank is read:** the act in play is the last one whose top has passed a
  line at 35% of the viewport height. Deliberately not an intersection band — a
  band leaves gaps between acts where nothing qualifies and the bill blinks
  empty on the way down.
- **Reduced motion:** the size step is removed entirely and both states settle
  back to the label voice (11.5px, `wdth` 92, `0.1em`). Rank survives as the
  inversion and the struck rule, which are not motion.

### Billing Rows (the model list)
- **Character:** each model is a line of the bill; the default one is billed.
- **Shape:** a three-column grid (6fr name, 7fr description, 5fr specs) on a
  shared baseline, separated by hairlines. No card, no border box.
- **Billed state:** the row inverts — ink ground, paper type — and bleeds 18px
  past the measure on both sides so the fill reads as a printed bar rather than
  a highlighted cell. Dimmed text and spec borders inside the row are re-declared
  against the dark ground.
- **Narrow:** collapses to one column at 1080px.

### Spec Chips
- **Style:** mono at 11.5px inside a 1px hairline rectangle, 3px/8px padding, no
  fill, no radius. They are factual labels (size, speed, language count), not
  tags and not buttons; there is no selected state.

### Ruled Key/Value Blocks
- **Style:** a hairline above the block, a faint hairline under each row, key in
  quiet ink on the left, value right-aligned. 11px rows.
- **Narrow:** stacks to two lines, value left-aligned.

### Plates (the app mocks)
- **Character:** the only images on the page, and the only proof it makes.
- **Style:** the app's own reproduced UI, mounted flat — radius, shadow and
  outline stripped, replaced by a 1px ink border.
- **Caption:** a `plate-note` strip under the plate — hairline above, the plate's
  name in the label voice, then a sentence of caption in quiet ink.
- **Furniture:** the overlay plate's progress chips and caption are page
  furniture around the plate, so they are set in the sheet's vocabulary (Archivo,
  hairlines, inversion for the active chip) rather than the app's.

### Docs Navigation
- **Style:** a sticky 218px left margin of grouped links, each group headed in
  the label voice over a hairline, each link under a faint hairline.
- **Hover:** oxblood. **Active:** inverted (ink ground, paper type) with 8px of
  horizontal padding so the fill reads as a bar.
- **Tracking:** driven by an IntersectionObserver with a
  `-24px 0px -70% 0px` root margin; the topmost visible section wins.
- **Narrow:** the margin unsticks and stacks above the body at 1080px.

### Icons
Inline SVG only, drawn on a 24px grid with 2px round-capped strokes, matching the
set the app itself uses. No icon font, no glyph characters, no icon tiles, no
icon-in-a-circle. On the page's own chrome only three appear: the arrow on a
ruled action, and the menu/close pair on the narrow-screen toggle.

## Do's and Don'ts

### Do:
- **Do** build every new surface out of bands divided by hairlines, inside the
  1360px measure with the 40px/22px gutter.
- **Do** carry rank with billing size and authored line breaks. If a new section
  needs to outrank its neighbour, give it a higher `.bill-` tier.
- **Do** use inversion — ink fill, ground-coloured type — as the only way to say
  active, current, featured or selected.
- **Do** reserve the width of an element's largest state whenever state changes
  its type size, so no row reflows.
- **Do** reach for a mechanical dot screen when a mid-tone is genuinely needed,
  at the established scales (1.6px on 9px, 1.15px on 5px).
- **Do** re-declare `--fg-2`, `--rule` and `--rule-faint` for any new ground, the
  way `.invert` does, instead of letting one value carry across both sheets.
- **Do** re-run any measurement-based layout on `document.fonts.ready`; a width
  measured against the fallback face is the wrong width.
- **Do** keep every text role at or above the 5.26:1 floor this build measured
  against its own ground.
- **Do** set commands, file sizes, spec values and timecodes in JetBrains Mono,
  and nothing else.

### Don't:
- **Don't** introduce a fifth colour, a gradient, a tint or an opacity-blended
  tone on the page's own chrome. The one gradient on the page lives inside the
  pinned brand logo (see Exceptions).
- **Don't** add a border radius, a shadow, a blur, a glass surface or a
  translucent layer. There is no elevation in this system.
- **Don't** build a card, a panel, a tile, a bordered box around body text or a
  trio of icon tiles. A filled panel never sits behind body text.
- **Don't** make a button a pill or a filled rounded block. Actions are type over
  a rule.
- **Don't** float the top run, shrink it on scroll, or give it a backdrop blur.
- **Don't** animate anything into existence. No `@keyframes`, no `animation`, no
  `transform`, no entrance, no scroll reveal, no parallax. Content is fully
  visible at all times, and the only authored motion changes rank.
- **Don't** print rank as a numeral, a kicker, an eyebrow, a step label or a
  coloured tag.
- **Don't** fill an area with oxblood or put it behind text; it is a contact and
  identity ink for type and rules.
- **Don't** add photography, illustration, decorative shapes, or any social
  proof — logos, counts, testimonials, stars. The app mocks are the only
  demonstration this site makes.
- **Don't** let a headline wrap on its own. Author the breaks per locale.
- **Don't** fold the mock palette or the mock faces into the page's system (see
  Exceptions).

### Exceptions (deliberate, scoped, and not to be generalised)

**The mocks are a foreign world, printed onto the sheet.** `css/mocks.css`,
`_includes/mocks/mocks.njk` and `js/mocks.js` are untouched reproductions of the
macOS app's own UI: its `.light` cream palette, its Space Grotesk and Hanken
Grotesk faces, its 14px rounded window, its shadows. They are not part of this
design system and their tokens are deliberately absent from this file. They are
scoped under `.mock` / `.overlay-stage`, and the sheet's only intervention is the
`.win` reset that mounts them as plates. A `.mock-` token is never a source for a
page decision, and a page token is never pushed into the mocks. `site.css` carries
one inline detector waiver for the `.mock`-scoped Space Grotesk face, which is
the honest record of that boundary.

**The logo is a pinned brand asset and carries gradients.** The world says no
gradient anywhere; the wordmark SVG has six. The mark is a pinned brand
commitment that predates this build and is used at 26px in the top line. It is
an imported asset, not a page surface, and it is the only place a gradient is
permitted.

**Raster provenance.** This build produced no shipping rasters. Every image on
the page is live HTML and CSS. The PNGs in `site/src/` — `og-en.png`,
`og-fr.png`, `apple-touch-icon.png`, `icon-192.png`, `icon-512.png` — are
pre-existing social-preview and favicon assets committed before this build; they
never render on the page. The remaining PNGs in the repo are review screenshots
under `.impeccable/review/`, which is gitignored.

### Divergences from the direction contract (recorded so the two agree from here)

**The headline sets in two lines, not three.** The FIRST VIEWPORT clause of the
direction contract asked for three. Three flush-set lines did fill the measure,
but they pushed the live-meeting plate out of the first viewport — and both the
filled measure and the plate-in-the-first-viewport were contract promises. Two
lines per locale keeps both. The finish reviewer endorsed the two-line setting
and asked that it be recorded here. Two authored lines per locale is now the
rule.

**The billing size-step plays in the bill, not on the act headings.** The
contract named the size-step on the act headings as the signature interaction.
The headings are full sentences that sit on a wrap boundary at almost any
measure, so stepping their size either gained a line — which made the emphasised
setting read *worse* than the quiet one — or, once capped to preserve the line
count, produced no visible step at all. The act headings therefore keep
permanent billing tiers, and the step, the demotion and the struck-off state
moved into the bill across the top, where each name reserves the width of its
largest state and the row holds still. The signature interaction is unchanged in
intent — the act in play takes top billing — and changed in where it is printed.
