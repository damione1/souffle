# Soufflé — marketing site

The landing page and documentation, as a static site. No backend, no
database: `npm run build` writes plain HTML into `_site/`, which is what
Cloudflare Pages serves.

## Running it

```bash
cd site
npm install
npm run dev     # http://localhost:8080, rebuilds on save
npm run build   # writes _site/
```

## Cloudflare Pages

| Setting | Value |
| --- | --- |
| Build command | `npm run build` |
| Build output directory | `_site` |
| Root directory | `site` |
| Node version | 20 or newer |

`src/_headers` is copied into the output, so Cloudflare picks up the
caching and security headers without extra configuration.

## Layout

```
src/
  _data/
    locales.js      the languages to build, one page per entry
    en.json         all English copy, landing + docs
    fr.json         all French copy, same keys
    site.json       URLs and constants shared by both
  _includes/
    layouts/base.njk
    partials/       nav, footer
    mocks/mocks.njk the app-screen reproductions
    icons.njk       icon and logo macros
  css/  site.css    page chrome · mocks.css  the app screens
  js/   site.js     copy buttons, reveal, docs scrollspy
        mocks.js    the animations
  index.njk         the landing page
  docs.njk          the documentation page
  src.11tydata.js   picks the locale bundle for each page
```

### Adding or changing copy

Everything a visitor reads lives in `src/_data/en.json` and
`src/_data/fr.json`. The two files have identical key structures, so a
change to one usually needs the same change to the other. Nothing else has
to be touched: templates loop over the data.

Check the two bundles still match after editing:

```bash
node -e '
const a=require("./src/_data/en.json"),b=require("./src/_data/fr.json");
const k=o=>{const s=new Set();(function w(v,p){if(v&&typeof v==="object"&&!Array.isArray(v))
for(const[x,y]of Object.entries(v)){s.add(p+"."+x);w(y,p+"."+x)}})(o,"");return s};
const A=k(a),B=k(b);
const miss=[...A].filter(x=>!B.has(x)),extra=[...B].filter(x=>!A.has(x));
console.log(miss.length||extra.length?{miss,extra}:"en/fr keys match");'
```

### Adding a language

Add an entry to `src/_data/locales.js` and a matching `src/_data/<code>.json`.
Both pages then build for it; the language switcher in `partials/nav.njk`
is the only place that still hardcodes EN and FR.

## The app screens

The mock screens are not screenshots. They are HTML and CSS built from the
real components, and they run the app's own behaviour:

| Mock | Built from | What animates |
| --- | --- | --- |
| Live meeting | `LiveSessionCard` (meeting), `MeetingNotesSection` | waveform, pulsing dot, elapsed clock, transcript committing a tentative tail |
| Home | `ActionHero`, `TimelineSection`, `TimelineItem` | static |
| Dictation | `LiveSessionCard` (dictation) | waveform, clock, text typing itself, blinking caret |
| Outcomes | `MeetingStructuredSummarySection` | static |
| Overlay | `PillApp` over a generic chat client | the full dictate → reformulate → paste cycle |

`js/mocks.js` ports the draw loop from `Waveform.svelte` (48 bars, 3 px wide,
2 px gap, same easing and alpha curve) and feeds it a synthetic speech
envelope instead of the backend's `AudioLevel` events.

Scenes idle until scrolled into view, stop when scrolled away or when the
tab is hidden, and render their finished state under
`prefers-reduced-motion: reduce`.

If a component in the app changes shape, the matching mock should be updated
with it. The values here were copied deliberately rather than approximated.

### The mocks currently run ahead of the app

They used to take their colours straight from the `.light` palette in
`src/app.css`. They no longer do. The screens now render the design the site
itself uses, in its Operate register: a warm white ground, panels as bands
between hairlines instead of cards with a fill and a shadow, inversion as the
whole state vocabulary, and one variable face.

That is a deliberate, temporary inversion of the usual contract. The mocks are
the specification for the app rather than a record of it, and the app is being
brought up to them. **Until it is, the site is showing a UI the shipped build
does not have.** When the app lands, this section goes and the palette is
sourced from `app.css` again.

The pill is the one part already verified against the real thing: its values
come from `src-tauri/swift/pill_panel.swift`, which replaced the original
implementation after these mocks were written.
