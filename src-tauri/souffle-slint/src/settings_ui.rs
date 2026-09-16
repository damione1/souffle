//! SOU-188: Settings shell. Mirrors `SettingsView.svelte` + its 17 sections,
//! one tab/section at a time as each milestone lands (see the ticket
//! journal for which). `AppSettings` is the same contract the real backend
//! (and the Svelte UI) already reads/writes - this never redeclares bounds
//! or option lists locally (AC3): `SettingsOptions::current()` is the single
//! source for those once a section needs them.

use crate::MainWindow;
use souffle_lib::logging::LogLevel;
use souffle_lib::settings::AppSettings;

/// Pushes `settings` into the Slint properties this shell currently wires.
/// Mirrors `controller.svelte.ts` setting `app.settings` after
/// `getSettings()`. Grows with each milestone; only the "Système" tab's
/// fields are populated so far.
pub fn populate(window: &MainWindow, settings: &AppSettings) {
    window.set_settings_autostart_enabled(settings.autostart_enabled);
    window.set_settings_debug_transcription(settings.debug_transcription);
    window.set_settings_log_level(settings.log_level.as_str().into());
}

/// Inverse of `LogLevel::as_str()` - the combobox in `diagnostics_section.slint`
/// only ever sends back one of these five values.
pub fn log_level_from_str(value: &str) -> LogLevel {
    match value {
        "error" => LogLevel::Error,
        "warn" => LogLevel::Warn,
        "debug" => LogLevel::Debug,
        "trace" => LogLevel::Trace,
        _ => LogLevel::Info,
    }
}
