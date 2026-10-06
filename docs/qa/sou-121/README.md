# SOU-121 native catalogue observations — 2026-10-05

`cargo run --manifest-path app/Cargo.toml -p souffle-slint --example apple_speech_catalog`
renders the production MainWindow/model widgets and uses the real catalogue and
`model_ui::populate_options` with the Apple selection. `SOU121_LOCALE=fr` or `en`
selects bundled translations. Skia renderer, 1040×820 logical content size.

The example opens no user profile, audio device or database and never installs or
deletes system assets. It is a rendering probe: model action callbacks are not
wired and it is not proof of the complete settings/download workflow.

- [French](catalog-fr.png): Apple Speech — assets système, Téléchargement requis.
- [English](catalog-en.png): Apple Speech — system assets, Download required.
- Both show the actual selected catalogue label and omit app-file deletion.
- Both native probe processes logged `Available { locale: "en_US", installed:
  false }`, `DownloadRequired`, `files_deletable=false`.

This native-binary status differs from the Rust test binary's earlier
`installed=true` and successful three-run real ASR probe documented in
`docs/lab/apple-speech.md`. No asset installation/removal was requested between
the probes. The reason for that process/time-dependent status remains unverified;
these screenshots do not establish installation/Ready in the shipped app.
Selection/install/load and live PTT/toggle+polish/system-mix acceptance remain
pending in the draft PR, along with approval of the continuous-silence policy.

The existing user Nightly process and its profile were preserved. Each isolated
example was closed normally after its window-only screenshot was inspected.
