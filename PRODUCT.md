# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

The design surface governed by this record is the marketing site and docs
(`site/`, Eleventy, static, Cloudflare Pages). The product it sells is a native
macOS app; the app's own UI lives in `src/` and is not designed by this record.

## Users

Two audiences, confirmed by the user, addressed by the same page:

- **Professionals in confidential meetings** (consultants, lawyers, doctors, HR).
  They cannot legally or ethically hand a recording to a third party. Privacy is
  a job constraint, not a preference. They are evaluating whether the tool is
  trustworthy enough to sit in a room where sensitive things are said.
- **Privacy-first technical users** (developers, people who refuse cloud on
  principle). They read the source before installing, care about the licence,
  the CLI and the MCP server, and are allergic to marketing claims they cannot
  verify.

Both arrive sceptical. Neither is looking to be impressed; they are looking for
a reason to disqualify the tool.

## Product Purpose

Soufflé turns speech into text on a Mac without the audio ever leaving the
machine: dictation into any app, live meeting transcription that separates the
user's voice from the other participants', and summaries with decisions and
action items. Success is a user dictating or recording a real meeting without
having to think about where the audio went.

## Positioning

The mechanism a cloud competitor cannot truthfully copy: **everything runs
on-device**. No account, no API key, no network call after the first model
download. Specifically:

- Speaker separation comes from native system-audio capture, with **no virtual
  audio device to install** — the usual price of admission for this category.
- Summaries run through Apple Intelligence or a local Ollama, never a hosted API.
- Open source (GPL-3.0-or-later), so the privacy claim is auditable rather than
  promised.

## Operating Context

- Apple Silicon Macs only; macOS 13+ for dictation, 14.4+ to capture other
  participants. There is no Intel and no Windows build.
- Used mid-meeting (video calls, in-person) and mid-task (dictating into chat,
  mail, an editor, a terminal) through a global shortcut and a floating overlay
  that stays out of screen recordings.
- Permissions (microphone, system audio, accessibility, calendar) are requested
  when a feature needs them, never up front.
- Distribution: signed and notarised `.dmg` from GitHub Releases, plus a personal
  Homebrew tap. Not in the Mac App Store.

## Capabilities and Constraints

- Four local speech models, downloaded on first use: Kyutai STT 1B (default,
  FR+EN, streaming), Kyutai STT 2.6B (EN, streaming), Whisper Large V3 Turbo
  (multilingual, on stop), Parakeet TDT 0.6B v3 (25 languages, CPU, on stop).
- Live streaming transcription, speaker lanes (Me / Them), inline correction that
  persists, optional local LLM reformulation with editable prompts, optional Opus
  audio retention, full-text search, export, an MCP server and a CLI.
- Free. No paid tier, no subscription, no licence to buy.
- Bilingual site (EN/FR), identical key structure in both copy bundles.

## Brand Commitments

Binding, confirmed by the user:

- The name **Soufflé** and the existing soufflé logo (`site/src/souffle-logo.svg`).
- The **warm amber** chromatic family. It comes from the app itself, and the
  animated mocks on the page render the app's real light palette — a site that
  drifted off amber would stop matching the product it shows.

Explicitly **not** binding: typography, layout, composition, motion, section
structure. The user asked for a blank canvas on everything except the above.

Voice: plain, factual, slightly dry. It states what the software does and what it
needs, and it does not sell. The existing copy is confirmed good and is reused.

## Evidence on Hand

- **Real and usable:** the animated mocks in `site/src/_includes/mocks/`. They are
  not screenshots — they are HTML/CSS rebuilt from the app's own components,
  running the app's own behaviour (the waveform loop is ported from
  `Waveform.svelte`). They are the product demonstration and the strongest asset
  on the page. The user asked explicitly to keep them.
- **Real and usable:** verifiable technical facts already on the page (model
  sizes, OS requirements, permissions, licence, signed/notarised releases).
- **Does not exist — must never be fabricated:** testimonials, user quotes,
  customer logos, download counts, star counts, star ratings, benchmark numbers,
  latency figures, accuracy percentages, "trusted by" claims, press mentions,
  awards. The user confirmed there is no social proof of any kind. The product
  demonstration is the only proof this site gets to make.

## Product Principles

1. **The claim must be checkable.** Every statement on the page is something a
   sceptic can verify in the source, in the release, or on their own machine.
2. **Show the software running, do not describe it.** The mocks carry the
   argument; prose supports them.
3. **Privacy is the mechanism, not the badge.** Explain how it is true (on-device,
   no virtual driver, open source), never assert it as a slogan.
4. **Do not oversell.** The audience is looking for a reason to disqualify the
   tool; a superlative gives them one.
5. **Both audiences on one page.** The professional needs reassurance, the
   developer needs detail. Detail is available, never in the way.

## Accessibility & Inclusion

- Keyboard reachable, visible focus, WCAG AA contrast on the dark ground.
- `prefers-reduced-motion: reduce` must render every animated mock in its
  finished state rather than animating — an existing, working behaviour that the
  redesign must preserve.
- Bilingual EN/FR with `hreflang`; French copy runs longer than English, so
  layouts cannot depend on English line lengths.
